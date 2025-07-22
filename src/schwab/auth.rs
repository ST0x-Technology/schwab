use base64::prelude::*;
use chrono::Utc;
use clap::Parser;
use reqwest::header::{self, HeaderMap, HeaderValue, InvalidHeaderValue};
use serde::Deserialize;
use thiserror::Error;

use super::tokens::SchwabTokens;

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

#[derive(Debug, Deserialize)]
pub struct SchwabAuthResponse {
    /// Expires every 30 minutes
    pub access_token: String,
    /// Expires every 7 days
    pub refresh_token: String,
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

        assert_eq!(env.redirect_uri, "https://127.0.0.1");
        assert_eq!(env.base_url, "https://api.schwabapi.com");
    }
}