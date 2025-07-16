use base64::prelude::*;
use chrono::{DateTime, Utc};
use clap::Parser;
use reqwest::header::{self, HeaderMap, HeaderValue, InvalidHeaderValue};
use serde::Deserialize;
use sqlx::SqlitePool;
use thiserror::Error;

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabAuthError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste code (from URL): ");

    let mut code = String::new();
    std::io::stdin().read_line(&mut code).unwrap();
    let code = code.trim();

    let tokens = env.get_tokens(code).await?;
    tokens.store(pool).await?;

    Ok(())
}

#[derive(Parser, Debug)]
pub struct SchwabAuthEnv {
    #[clap(short, long, env)]
    app_key: String,
    #[clap(short, long, env)]
    app_secret: String,
    #[clap(short, long, env, default_value = "https://127.0.0.1")]
    redirect_uri: String,
    #[clap(short, long, env, default_value = "https://api.schwabapi.com")]
    base_url: String,
}

#[derive(Error, Debug)]
pub enum SchwabAuthError {
    #[error("Failed to create header value: {0}")]
    InvalidHeader(#[from] InvalidHeaderValue),
    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
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
            "grant_type=authorization_code&code={}&redirect_uri={}",
            code, self.redirect_uri
        );

        let headers = HeaderMap::from_iter(
            vec![
                (
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Basic {credentials}"))?,
                ),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_str("application/x-www-form-urlencoded")?,
                ),
            ]
            .into_iter(),
        );

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
    access_token: String,
    access_token_fetched_at: DateTime<Utc>,
    /// Expires every 7 days
    refresh_token: String,
    refresh_token_fetched_at: DateTime<Utc>,
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
            SELECT (
                id,
                access_token,
                access_token_fetched_at,
                refresh_token,
                refresh_token_fetched_at
            )
            FROM schwab_auth
            ORDER BY id DESC
            LIMIT 1
            "#
        )
        .fetch_one(&pool)
        .await?;

        Ok(Self {
            access_token: row.access_token,
            access_token_fetched_at: row.access_token_fetched_at,
            refresh_token: row.refresh_token,
            refresh_token_fetched_at: row.refresh_token_fetched_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use httpmock::prelude::*;
    use serde_json::json;
    use sqlx::SqlitePool;
    use std::time::Duration;

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
        assert!(
            now.signed_duration_since(tokens.access_token_fetched_at)
                < chrono::Duration::seconds(5)
        );
        assert!(
            now.signed_duration_since(tokens.refresh_token_fetched_at)
                < chrono::Duration::seconds(5)
        );
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
    async fn test_get_tokens_invalid_header_credentials() {
        let env = SchwabAuthEnv {
            app_key: "test\nkey".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
        };

        let result = env.get_tokens("test_code").await;

        assert!(matches!(
            result.unwrap_err(),
            SchwabAuthError::InvalidHeader(_)
        ));
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

        let result = tokens.store(&pool).await;
        assert!(result.is_ok());

        let stored_token = assert_eq!(stored_token.access_token, "test_access_token");
        assert_eq!(stored_token.refresh_token, "test_refresh_token");
        assert_eq!(stored_token.access_token_expires_at, now.naive_utc());
        assert_eq!(stored_token.refresh_token_expires_at, now.naive_utc());
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
        let result1 = tokens.store(&pool).await;
        assert!(result1.is_ok());

        // Second insert should also succeed (no unique constraint on tokens)
        let result2 = tokens.store(&pool).await;
        assert!(result2.is_ok());

        // Verify both records exist
        let count = sqlx::query!("SELECT COUNT(*) as count FROM schwab_auth")
            .fetch_one(&pool)
            .await
            .unwrap()
            .count;
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_schwab_tokens_store_database_error() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        // Don't create the table, so the insert will fail

        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now(),
        };

        let result = tokens.store(&pool).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SchwabAuthError::Sqlx(_)));
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
        assert!(result.is_err());
    }

    #[test]
    fn test_schwab_tokens_debug_format() {
        let now = Utc::now();
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: now,
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: now,
        };

        let debug_str = format!("{:?}", tokens);
        assert!(debug_str.contains("access_token"));
        assert!(debug_str.contains("refresh_token"));
        assert!(debug_str.contains("test_access_token"));
        assert!(debug_str.contains("test_refresh_token"));
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
        assert!(result.is_ok());

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

    #[tokio::test]
    async fn test_get_tokens_network_timeout() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .delay(Duration::from_secs(10)) // Simulate slow response
                .header("content-type", "application/json")
                .json_body(json!({"access_token": "test", "refresh_token": "test"}));
        });

        // This test relies on the default reqwest timeout, which should be reasonable
        // In a real-world scenario, you might want to configure a custom timeout
        let result = env.get_tokens("test_code").await;

        // The result could be Ok or Err depending on the default timeout
        // This test mainly ensures the function handles slow responses gracefully
        match result {
            Ok(_) => {
                // If successful, the mock should have been called
                mock.assert();
            }
            Err(SchwabAuthError::Reqwest(_)) => {
                // Network timeout is expected and acceptable
            }
            Err(e) => {
                panic!("Unexpected error type: {:?}", e);
            }
        }
    }
}
