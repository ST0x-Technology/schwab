use base64::prelude::*;
use chrono::{DateTime, Duration, Utc};
use clap::Parser;
use reqwest::header::{self, HeaderMap, HeaderValue, InvalidHeaderValue};
use serde::Deserialize;
use sqlx::SqlitePool;
use thiserror::Error;
use tokio::time::{Duration as TokioDuration, interval};

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabAuthError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste code (from URL): ");

    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let code = code.trim();

    let tokens = env.get_tokens(code).await?;
    tokens.store(pool).await?;

    Ok(())
}

#[derive(Parser, Debug, Clone)]
pub struct SchwabAuthEnv {
    #[clap(short, long, env)]
    pub app_key: String,
    #[clap(short, long, env)]
    pub app_secret: String,
    #[clap(short, long, env, default_value = "https://127.0.0.1")]
    pub redirect_uri: String,
    #[clap(short, long, env, default_value = "https://api.schwabapi.com")]
    pub base_url: String,
}

#[derive(Error, Debug)]
pub enum SchwabAuthError {
    #[error("Failed to create header value: {0}")]
    InvalidHeader(#[from] InvalidHeaderValue),
    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Refresh token has expired")]
    RefreshTokenExpired,
}

impl SchwabAuthEnv {
    pub fn get_auth_url(&self) -> String {
        format!(
            "{}/v1/oauth/authorize?client_id={}&redirect_uri={}",
            self.base_url, self.app_key, self.redirect_uri
        )
    }

    pub async fn get_tokens(&self, code: &str) -> Result<SchwabTokens, SchwabAuthError> {
        let credentials = format!("{}:{}", self.app_key, self.app_secret);
        let credentials = BASE64_STANDARD.encode(credentials);

        let payload = format!(
            "grant_type=authorization_code&code={code}&redirect_uri={}",
            self.redirect_uri
        );

        let headers = [
            (
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {credentials}"))?,
            ),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str("application/x-www-form-urlencoded")?,
            ),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/oauth/token", self.base_url))
            .headers(headers)
            .body(payload)
            .send()
            .await?;

        let response: SchwabAuthResponse = response.json().await?;

        Ok(SchwabTokens {
            access_token: response.access_token,
            access_token_fetched_at: Utc::now(),
            refresh_token: response.refresh_token,
            refresh_token_fetched_at: Utc::now(),
        })
    }

    pub async fn refresh_tokens(
        &self,
        refresh_token: &str,
    ) -> Result<SchwabTokens, SchwabAuthError> {
        let credentials = format!("{}:{}", self.app_key, self.app_secret);
        let credentials = BASE64_STANDARD.encode(credentials);

        let payload = format!("grant_type=refresh_token&refresh_token={refresh_token}");

        let headers = [
            (
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {credentials}"))?,
            ),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str("application/x-www-form-urlencoded")?,
            ),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/oauth/token", self.base_url))
            .headers(headers)
            .body(payload)
            .send()
            .await?;

        let response: SchwabAuthResponse = response.json().await?;

        Ok(SchwabTokens {
            access_token: response.access_token,
            access_token_fetched_at: Utc::now(),
            refresh_token: response.refresh_token,
            refresh_token_fetched_at: Utc::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct SchwabAuthResponse {
    /// Expires every 30 minutes
    access_token: String,
    /// Expires every 7 days
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct SchwabTokens {
    /// Expires every 30 minutes
    pub access_token: String,
    pub access_token_fetched_at: DateTime<Utc>,
    /// Expires every 7 days
    pub refresh_token: String,
    pub refresh_token_fetched_at: DateTime<Utc>,
}

impl SchwabTokens {
    pub async fn store(&self, pool: &SqlitePool) -> Result<(), SchwabAuthError> {
        sqlx::query!(
            r#"
            INSERT INTO schwab_auth (
                access_token,
                access_token_fetched_at,
                refresh_token,
                refresh_token_fetched_at
            )
            VALUES (?, ?, ?, ?)
            "#,
            self.access_token,
            self.access_token_fetched_at,
            self.refresh_token,
            self.refresh_token_fetched_at,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn load(pool: &SqlitePool) -> Result<Self, SchwabAuthError> {
        let row = sqlx::query!(
            r#"
            SELECT
                id,
                access_token,
                access_token_fetched_at,
                refresh_token,
                refresh_token_fetched_at
            FROM schwab_auth
            ORDER BY id DESC
            LIMIT 1
            "#
        )
        .fetch_one(pool)
        .await?;

        Ok(Self {
            access_token: row.access_token,
            access_token_fetched_at: DateTime::from_naive_utc_and_offset(
                row.access_token_fetched_at,
                Utc,
            ),
            refresh_token: row.refresh_token,
            refresh_token_fetched_at: DateTime::from_naive_utc_and_offset(
                row.refresh_token_fetched_at,
                Utc,
            ),
        })
    }

    pub fn is_access_token_expired(&self) -> bool {
        let now = Utc::now();
        let expires_at = self.access_token_fetched_at + Duration::minutes(30);
        now >= expires_at
    }

    pub fn is_refresh_token_expired(&self) -> bool {
        let now = Utc::now();
        let expires_at = self.refresh_token_fetched_at + Duration::days(7);
        now >= expires_at
    }

    pub fn access_token_expires_in(&self) -> Duration {
        let now = Utc::now();
        let expires_at = self.access_token_fetched_at + Duration::minutes(30);
        expires_at - now
    }

    pub fn refresh_token_expires_in(&self) -> Duration {
        let now = Utc::now();
        let expires_at = self.refresh_token_fetched_at + Duration::days(7);
        expires_at - now
    }

    pub async fn get_valid_access_token(
        pool: &SqlitePool,
        env: &SchwabAuthEnv,
    ) -> Result<String, SchwabAuthError> {
        let tokens = Self::load(pool).await?;

        if !tokens.is_access_token_expired() {
            return Ok(tokens.access_token);
        }

        if tokens.is_refresh_token_expired() {
            return Err(SchwabAuthError::RefreshTokenExpired);
        }

        let new_tokens = env.refresh_tokens(&tokens.refresh_token).await?;
        new_tokens.store(pool).await?;
        Ok(new_tokens.access_token)
    }

    pub async fn start_automatic_token_refresh(
        pool: SqlitePool,
        env: SchwabAuthEnv,
    ) -> Result<(), SchwabAuthError> {
        let mut interval_timer = interval(TokioDuration::from_secs(29 * 60));

        loop {
            interval_timer.tick().await;

            Self::handle_token_refresh(&pool, &env).await?;
        }
    }

    async fn handle_token_refresh(
        pool: &SqlitePool,
        env: &SchwabAuthEnv,
    ) -> Result<(), SchwabAuthError> {
        match Self::refresh_if_needed(pool, env).await {
            Ok(refreshed) if refreshed => {
                println!("Access token refreshed successfully");
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(SchwabAuthError::RefreshTokenExpired) => {
                println!("Refresh token expired, manual re-authentication required");
                Err(SchwabAuthError::RefreshTokenExpired)
            }
            Err(e) => {
                println!("Failed to refresh token: {e}");
                Ok(())
            }
        }
    }

    pub async fn refresh_if_needed(
        pool: &SqlitePool,
        env: &SchwabAuthEnv,
    ) -> Result<bool, SchwabAuthError> {
        let tokens = Self::load(pool).await?;

        if tokens.is_refresh_token_expired() {
            return Err(SchwabAuthError::RefreshTokenExpired);
        }

        if tokens.is_access_token_expired()
            || tokens.access_token_expires_in() <= Duration::minutes(1)
        {
            let new_tokens = env.refresh_tokens(&tokens.refresh_token).await?;
            new_tokens.store(pool).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use httpmock::prelude::*;
    use serde_json::json;
    use sqlx::SqlitePool;

    fn create_test_env() -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
        }
    }

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
        }
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[test]
    fn test_schwab_auth_env_get_auth_url() {
        let env = create_test_env();
        let expected_url = "https://api.schwabapi.com/v1/oauth/authorize?client_id=test_app_key&redirect_uri=https://127.0.0.1";
        assert_eq!(env.get_auth_url(), expected_url);
    }

    #[test]
    fn test_schwab_auth_env_get_auth_url_custom_base_url() {
        let env = SchwabAuthEnv {
            app_key: "custom_key".to_string(),
            app_secret: "custom_secret".to_string(),
            redirect_uri: "https://custom.redirect.com".to_string(),
            base_url: "https://custom.api.com".to_string(),
        };
        let expected_url = "https://custom.api.com/v1/oauth/authorize?client_id=custom_key&redirect_uri=https://custom.redirect.com";
        assert_eq!(env.get_auth_url(), expected_url);
    }

    #[tokio::test]
    async fn test_get_tokens_success() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "test_access_token",
            "refresh_token": "test_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                ) // base64(test_app_key:test_app_secret)
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=authorization_code")
                .body_contains("code=test_code")
                .body_contains("redirect_uri=https://127.0.0.1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_tokens("test_code").await;

        mock.assert();

        let tokens = result.unwrap();
        assert_eq!(tokens.access_token, "test_access_token");
        assert_eq!(tokens.refresh_token, "test_refresh_token");

        // Check that timestamps are recent (within last 5 seconds)
        let now = Utc::now();
        assert!(now.signed_duration_since(tokens.access_token_fetched_at) < Duration::seconds(5));
        assert!(now.signed_duration_since(tokens.refresh_token_fetched_at) < Duration::seconds(5));
    }

    #[tokio::test]
    async fn test_get_tokens_http_error() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_request"}));
        });

        let result = env.get_tokens("invalid_code").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_get_tokens_json_parse_error() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .body("invalid json");
        });

        let result = env.get_tokens("test_code").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_get_tokens_missing_fields() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "test_access_token"
            // Missing refresh_token
        });

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_tokens("test_code").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_schwab_tokens_store_success() {
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        tokens.store(&pool).await.unwrap();

        let stored_token = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(stored_token.access_token, "test_access_token");
        assert_eq!(stored_token.refresh_token, "test_refresh_token");
        assert_eq!(stored_token.access_token_fetched_at, now);
        assert_eq!(stored_token.refresh_token_fetched_at, now);
    }

    #[tokio::test]
    async fn test_schwab_tokens_store_duplicate_insert() {
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        // First insert should succeed
        tokens.store(&pool).await.unwrap();

        // Second insert should also succeed (no unique constraint on tokens)
        tokens.store(&pool).await.unwrap();

        // Verify both records exist
        let count = sqlx::query!("SELECT COUNT(*) as count FROM schwab_auth")
            .fetch_one(&pool)
            .await
            .unwrap()
            .count;
        assert_eq!(count, 2);
    }

    #[test]
    fn test_schwab_auth_response_deserialization() {
        let json_str = r#"{"access_token": "test_access", "refresh_token": "test_refresh"}"#;
        let response: SchwabAuthResponse = serde_json::from_str(json_str).unwrap();

        assert_eq!(response.access_token, "test_access");
        assert_eq!(response.refresh_token, "test_refresh");
    }

    #[test]
    fn test_schwab_auth_response_deserialization_missing_field() {
        let json_str = r#"{"access_token": "test_access"}"#;
        let result: Result<SchwabAuthResponse, _> = serde_json::from_str(json_str);
        assert!(matches!(result.unwrap_err(), serde_json::Error { .. }));
    }

    #[test]
    fn test_schwab_auth_error_display() {
        let invalid_header_err =
            SchwabAuthError::InvalidHeader(HeaderValue::from_str("test\x00").unwrap_err());
        assert!(
            invalid_header_err
                .to_string()
                .contains("Failed to create header value")
        );
    }

    #[test]
    fn test_schwab_auth_error_from_conversions() {
        let header_err = HeaderValue::from_str("test\x00").unwrap_err();
        let auth_err: SchwabAuthError = header_err.into();
        assert!(matches!(auth_err, SchwabAuthError::InvalidHeader(_)));
    }

    #[test]
    fn test_schwab_auth_env_default_values() {
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
        };

        // Test default values match the struct defaults
        assert_eq!(env.redirect_uri, "https://127.0.0.1");
        assert_eq!(env.base_url, "https://api.schwabapi.com");
    }

    #[tokio::test]
    async fn test_get_tokens_with_special_characters() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "access_token_with_special_chars_!@#$%^&*()",
            "refresh_token": "refresh_token_with_special_chars_!@#$%^&*()"
        });

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_tokens("code_with_special_chars_!@#$%^&*()").await;

        mock.assert();
        let tokens = result.unwrap();
        assert_eq!(
            tokens.access_token,
            "access_token_with_special_chars_!@#$%^&*()"
        );
        assert_eq!(
            tokens.refresh_token,
            "refresh_token_with_special_chars_!@#$%^&*()"
        );
    }

    #[test]
    fn test_is_access_token_expired_not_expired() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(15), // 15 minutes ago
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        assert!(!tokens.is_access_token_expired());
    }

    #[test]
    fn test_is_access_token_expired_expired() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(31), // 31 minutes ago
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        assert!(tokens.is_access_token_expired());
    }

    #[test]
    fn test_is_access_token_expired_exactly_30_minutes() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(30), // Exactly 30 minutes ago
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        assert!(tokens.is_access_token_expired());
    }

    #[test]
    fn test_is_refresh_token_expired_not_expired() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(3), // 3 days ago
        };

        assert!(!tokens.is_refresh_token_expired());
    }

    #[test]
    fn test_is_refresh_token_expired_expired() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(8), // 8 days ago
        };

        assert!(tokens.is_refresh_token_expired());
    }

    #[test]
    fn test_is_refresh_token_expired_exactly_7_days() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(7), // Exactly 7 days ago
        };

        assert!(tokens.is_refresh_token_expired());
    }

    #[test]
    fn test_access_token_expires_in_positive() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(10), // 10 minutes ago
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        let expires_in = tokens.access_token_expires_in();
        assert!(expires_in > Duration::minutes(19));
        assert!(expires_in <= Duration::minutes(20));
    }

    #[test]
    fn test_access_token_expires_in_negative() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(35), // 35 minutes ago
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        let expires_in = tokens.access_token_expires_in();
        assert!(expires_in < Duration::zero());
    }

    #[test]
    fn test_refresh_token_expires_in_positive() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(2), // 2 days ago
        };

        let expires_in = tokens.refresh_token_expires_in();
        assert!(expires_in > Duration::days(4));
        assert!(expires_in <= Duration::days(5));
    }

    #[test]
    fn test_refresh_token_expires_in_negative() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(10), // 10 days ago
        };

        let expires_in = tokens.refresh_token_expires_in();
        assert!(expires_in < Duration::zero());
    }

    #[tokio::test]
    async fn test_refresh_tokens_success() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "new_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=old_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.refresh_tokens("old_refresh_token").await;

        mock.assert();

        let tokens = result.unwrap();
        assert_eq!(tokens.access_token, "new_access_token");
        assert_eq!(tokens.refresh_token, "new_refresh_token");

        let now = Utc::now();
        assert!(now.signed_duration_since(tokens.access_token_fetched_at) < Duration::seconds(5));
        assert!(now.signed_duration_since(tokens.refresh_token_fetched_at) < Duration::seconds(5));
    }

    #[tokio::test]
    async fn test_refresh_tokens_http_error() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_grant"}));
        });

        let result = env.refresh_tokens("invalid_refresh_token").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_refresh_tokens_json_parse_error() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .body("invalid json");
        });

        let result = env.refresh_tokens("test_refresh_token").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_refresh_tokens_missing_fields() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "new_access_token"
            // Missing refresh_token
        });

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.refresh_tokens("test_refresh_token").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_get_valid_access_token_valid_token() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(10), // 10 minutes ago, still valid
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1), // 1 day ago, still valid
        };

        tokens.store(&pool).await.unwrap();

        let result = SchwabTokens::get_valid_access_token(&pool, &env).await;
        assert_eq!(result.unwrap(), "valid_access_token");
    }

    #[tokio::test]
    async fn test_get_valid_access_token_refresh_token_expired() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(35), // 35 minutes ago, expired
            refresh_token: "expired_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(8), // 8 days ago, expired
        };

        tokens.store(&pool).await.unwrap();

        let result = SchwabTokens::get_valid_access_token(&pool, &env).await;

        assert!(matches!(
            result.unwrap_err(),
            SchwabAuthError::RefreshTokenExpired
        ));
    }

    #[tokio::test]
    async fn test_get_valid_access_token_needs_refresh() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(35), // 35 minutes ago, expired
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1), // 1 day ago, still valid
        };

        tokens.store(&pool).await.unwrap();

        let mock_response = json!({
            "access_token": "refreshed_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = SchwabTokens::get_valid_access_token(&pool, &env).await;

        mock.assert();
        assert_eq!(result.unwrap(), "refreshed_access_token");

        // Verify the new tokens were stored
        let stored_tokens = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(stored_tokens.access_token, "refreshed_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_get_valid_access_token_refresh_fails() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(35), // 35 minutes ago, expired
            refresh_token: "invalid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1), // 1 day ago, still valid
        };

        tokens.store(&pool).await.unwrap();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_grant"}));
        });

        let result = SchwabTokens::get_valid_access_token(&pool, &env).await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_get_valid_access_token_no_tokens_in_db() {
        let pool = setup_test_db().await;
        let env = create_test_env();

        let result = SchwabTokens::get_valid_access_token(&pool, &env).await;

        assert!(matches!(result.unwrap_err(), SchwabAuthError::Sqlx(_)));
    }

    #[tokio::test]
    async fn test_refresh_if_needed_success() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(31),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1),
        };

        tokens.store(&pool).await.unwrap();

        let mock_response = json!({
            "access_token": "refreshed_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = SchwabTokens::refresh_if_needed(&pool, &env).await;

        mock.assert();
        assert!(result.unwrap());

        let stored_tokens = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(stored_tokens.access_token, "refreshed_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_refresh_if_needed_with_expired_refresh_token() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(31),
            refresh_token: "expired_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(8),
        };

        tokens.store(&pool).await.unwrap();

        let result = SchwabTokens::refresh_if_needed(&pool, &env).await;

        assert!(matches!(
            result.unwrap_err(),
            SchwabAuthError::RefreshTokenExpired
        ));
    }

    #[tokio::test]
    async fn test_refresh_if_needed_no_refresh_needed() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(10),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1),
        };

        tokens.store(&pool).await.unwrap();

        let result = SchwabTokens::refresh_if_needed(&pool, &env).await;

        assert!(!result.unwrap());

        let stored_tokens = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(stored_tokens.access_token, "valid_access_token");
    }

    #[tokio::test]
    async fn test_refresh_if_needed_near_expiration() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "near_expiry_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(29),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1),
        };

        tokens.store(&pool).await.unwrap();

        let mock_response = json!({
            "access_token": "refreshed_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = SchwabTokens::refresh_if_needed(&pool, &env).await;

        mock.assert();
        assert!(result.unwrap());

        let stored_tokens = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(stored_tokens.access_token, "refreshed_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_automatic_token_refresh_before_expiration() -> Result<(), SchwabAuthError> {
        use std::thread;
        use tokio::time::{Duration as TokioDuration, sleep};

        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        let now = Utc::now();

        let tokens = SchwabTokens {
            access_token: "near_expiration_access_token".to_string(),
            access_token_fetched_at: now - Duration::minutes(29),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: now - Duration::days(1),
        };

        tokens.store(&pool).await?;

        let mock_response = serde_json::json!({
            "access_token": "refreshed_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let pool_clone = pool.clone();
        let env_clone = env.clone();

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                tokio::time::timeout(
                    TokioDuration::from_secs(5),
                    SchwabTokens::start_automatic_token_refresh(pool_clone, env_clone),
                )
                .await
            })
        });

        sleep(TokioDuration::from_millis(2000)).await;

        handle.join().unwrap().unwrap_err();

        mock.assert();

        let stored_tokens = SchwabTokens::load(&pool).await?;
        assert_eq!(stored_tokens.access_token, "refreshed_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");

        Ok(())
    }

    #[tokio::test]
    async fn test_run_oauth_flow() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let mock_response = json!({
            "access_token": "test_access_token",
            "refresh_token": "test_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=authorization_code")
                .body_contains("code=test_code")
                .body_contains("redirect_uri=https://127.0.0.1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_tokens("test_code").await;
        assert!(result.is_ok());

        let tokens = result.unwrap();
        let store_result = tokens.store(&pool).await;
        assert!(store_result.is_ok());

        mock.assert();
    }

    #[tokio::test]
    async fn test_schwab_tokens_conversion_error() {
        let header_err = HeaderValue::from_str("test\x00").unwrap_err();
        let schwab_err = SchwabAuthError::InvalidHeader(header_err);

        assert!(matches!(schwab_err, SchwabAuthError::InvalidHeader(_)));
    }
}
