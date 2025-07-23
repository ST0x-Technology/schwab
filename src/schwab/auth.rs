use base64::prelude::*;
use chrono::Utc;
use clap::Parser;
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::{SchwabError, tokens::SchwabTokens};

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
    #[clap(long, env, default_value = "0")]
    pub account_index: usize,
}

#[derive(Debug, Deserialize)]
pub struct SchwabAuthResponse {
    /// Expires every 30 minutes
    pub access_token: String,
    /// Expires every 7 days
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountNumbers {
    pub account_number: String,
    pub hash_value: String,
}

impl SchwabAuthEnv {
    pub async fn get_account_hash(&self, pool: &SqlitePool) -> Result<String, SchwabError> {
        let access_token = SchwabTokens::get_valid_access_token(pool, self).await?;

        let headers = [
            (
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {access_token}"))?,
            ),
            (header::ACCEPT, HeaderValue::from_str("application/json")?),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "{}/trader/v1/accounts/accountNumbers",
                self.base_url
            ))
            .headers(headers)
            .send()
            .await?;

        let account_numbers: Vec<AccountNumbers> = response.json().await?;

        if account_numbers.is_empty() {
            return Err(SchwabError::NoAccountsFound);
        }

        if self.account_index >= account_numbers.len() {
            return Err(SchwabError::AccountIndexOutOfBounds {
                index: self.account_index,
                count: account_numbers.len(),
            });
        }

        Ok(account_numbers[self.account_index].hash_value.clone())
    }

    pub fn get_auth_url(&self) -> String {
        format!(
            "{}/v1/oauth/authorize?client_id={}&redirect_uri={}",
            self.base_url, self.app_key, self.redirect_uri
        )
    }

    pub async fn get_tokens(&self, code: &str) -> Result<SchwabTokens, SchwabError> {
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

    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<SchwabTokens, SchwabError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use httpmock::prelude::*;
    use serde_json::json;

    fn create_test_env() -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
            account_index: 0,
        }
    }

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
            account_index: 0,
        }
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
            account_index: 0,
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

        mock.assert();

        let tokens = result.unwrap();
        assert_eq!(tokens.access_token, "test_access_token");
        assert_eq!(tokens.refresh_token, "test_refresh_token");

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
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
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
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_get_tokens_missing_fields() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "test_access_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_tokens("test_code").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
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
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
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
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_refresh_tokens_missing_fields() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock_response = json!({
            "access_token": "new_access_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.refresh_tokens("test_refresh_token").await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
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
            SchwabError::InvalidHeader(HeaderValue::from_str("test\x00").unwrap_err());
        assert!(
            invalid_header_err
                .to_string()
                .contains("Failed to create header value")
        );
    }

    #[test]
    fn test_schwab_auth_env_default_values() {
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
            account_index: 0,
        };

        assert_eq!(env.redirect_uri, "https://127.0.0.1");
        assert_eq!(env.base_url, "https://api.schwabapi.com");
    }

    #[tokio::test]
    async fn test_get_account_hash_success() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock_response = json!([
            {
                "accountNumber": "123456789",
                "hashValue": "ABC123DEF456"
            },
            {
                "accountNumber": "987654321",
                "hashValue": "XYZ789GHI012"
            }
        ]);

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/trader/v1/accounts/accountNumbers")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_account_hash(&pool).await;

        mock.assert();
        assert_eq!(result.unwrap(), "ABC123DEF456");
    }

    #[tokio::test]
    async fn test_get_account_hash_with_custom_index() {
        let server = MockServer::start();
        let mut env = create_test_env_with_mock_server(&server);
        env.account_index = 1;
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock_response = json!([
            {
                "accountNumber": "123456789",
                "hashValue": "ABC123DEF456"
            },
            {
                "accountNumber": "987654321",
                "hashValue": "XYZ789GHI012"
            }
        ]);

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/trader/v1/accounts/accountNumbers")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_account_hash(&pool).await;

        mock.assert();
        assert_eq!(result.unwrap(), "XYZ789GHI012");
    }

    #[tokio::test]
    async fn test_get_account_hash_index_out_of_bounds() {
        let server = MockServer::start();
        let mut env = create_test_env_with_mock_server(&server);
        env.account_index = 2;
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock_response = json!([
            {
                "accountNumber": "123456789",
                "hashValue": "ABC123DEF456"
            }
        ]);

        let mock = server.mock(|when, then| {
            when.method(GET).path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let result = env.get_account_hash(&pool).await;

        mock.assert();
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::AccountIndexOutOfBounds { index: 2, count: 1 }
        ));
    }

    #[tokio::test]
    async fn test_get_account_hash_no_accounts() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock = server.mock(|when, then| {
            when.method(GET).path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([]));
        });

        let result = env.get_account_hash(&pool).await;

        mock.assert();
        assert!(matches!(result.unwrap_err(), SchwabError::NoAccountsFound));
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    async fn setup_test_tokens(pool: &SqlitePool) {
        let tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(pool).await.unwrap();
    }
}
