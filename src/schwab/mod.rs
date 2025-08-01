use reqwest::header::InvalidHeaderValue;
use sqlx::SqlitePool;
use std::io::{self, Write};
use thiserror::Error;

pub mod auth;
pub mod order;
pub mod tokens;

pub use auth::{AccountNumbers, SchwabAuthEnv, SchwabAuthResponse};
pub use tokens::SchwabTokens;

#[derive(Error, Debug)]
pub enum SchwabError {
    #[error("Failed to create header value: {0}")]
    InvalidHeader(#[from] InvalidHeaderValue),
    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("URL parsing failed: {0}")]
    Url(#[from] url::ParseError),
    #[error("Missing authorization code parameter in URL: {url}")]
    MissingAuthCode { url: String },
    #[error("JSON serialization failed: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("Refresh token has expired")]
    RefreshTokenExpired,
    #[error("No accounts found")]
    NoAccountsFound,
    #[error("Account index {index} out of bounds (found {count} accounts)")]
    AccountIndexOutOfBounds { index: usize, count: usize },
    #[error("{action} failed with status: {status}, body: {body}")]
    RequestFailed {
        action: String,
        status: reqwest::StatusCode,
        body: String,
    },
}

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste the full redirect URL you were sent to: ");
    io::stdout().flush()?;

    let mut redirect_url = String::new();
    io::stdin().read_line(&mut redirect_url)?;
    let redirect_url = redirect_url.trim();

    let code = extract_code_from_url(redirect_url)?;
    println!("Extracted code: {code}");

    let tokens = env.get_tokens_from_code(&code).await?;
    tokens.store(pool).await?;

    Ok(())
}

fn extract_code_from_url(url: &str) -> Result<String, SchwabError> {
    let parsed_url = url::Url::parse(url)?;

    parsed_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| SchwabError::MissingAuthCode {
            url: url.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;
    use sqlx::SqlitePool;

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
            account_index: 0,
        }
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
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
                .body_contains("redirect_uri=https%3A%2F%2F127.0.0.1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let tokens = env.get_tokens_from_code("test_code").await.unwrap();
        tokens.store(&pool).await.unwrap();

        mock.assert();
    }

    #[test]
    fn test_extract_code_from_url_success() {
        let url = "https://127.0.0.1/?code=test_auth_code&state=xyz";
        let result = extract_code_from_url(url);
        assert_eq!(result.unwrap(), "test_auth_code");
    }

    #[test]
    fn test_extract_code_from_url_missing_code() {
        let url = "https://127.0.0.1/?state=xyz&other=param";
        let result = extract_code_from_url(url);
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::MissingAuthCode { url: ref u } if u == "https://127.0.0.1/?state=xyz&other=param"
        ));
    }

    #[test]
    fn test_extract_code_from_url_invalid_url() {
        let url = "not_a_valid_url";
        let result = extract_code_from_url(url);
        assert!(matches!(result.unwrap_err(), SchwabError::Url(_)));
    }

    #[test]
    fn test_extract_code_from_url_no_query_params() {
        let url = "https://127.0.0.1/";
        let result = extract_code_from_url(url);
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::MissingAuthCode { url: ref u } if u == "https://127.0.0.1/"
        ));
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_http_401() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=authorization_code")
                .body_contains("code=invalid_code");
            then.status(401)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_grant"}));
        });

        let result = env.get_tokens_from_code("invalid_code").await;
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "get tokens" && status.as_u16() == 401
        ));

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_http_500() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(500);
        });

        let result = env.get_tokens_from_code("test_code").await;
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "get tokens" && status.as_u16() == 500
        ));

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_invalid_json_response() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .body("invalid json");
        });

        let result = env.get_tokens_from_code("test_code").await;
        assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));

        mock.assert();
    }
}
