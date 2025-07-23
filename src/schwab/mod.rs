pub mod auth;
pub mod order;
pub mod tokens;

use reqwest::header::InvalidHeaderValue;
use sqlx::SqlitePool;
use std::io;
use thiserror::Error;

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
    #[error("JSON serialization failed: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("Refresh token has expired")]
    RefreshTokenExpired,
    #[error("No accounts found")]
    NoAccountsFound,
    #[error("Account index {index} out of bounds (found {count} accounts)")]
    AccountIndexOutOfBounds { index: usize, count: usize },
    #[error("Order placement failed with status: {status}")]
    OrderPlacementFailed { status: reqwest::StatusCode },
    #[error("Account hash retrieval failed with status: {status}, body: {body}")]
    AccountHashRetrievalFailed {
        status: reqwest::StatusCode,
        body: String,
    },
}

pub use auth::{AccountNumbers, SchwabAuthEnv, SchwabAuthResponse};
pub use tokens::SchwabTokens;

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste code (from URL): ");

    let mut code = String::new();
    io::stdin().read_line(&mut code)?;
    let code = code.trim();

    let tokens = env.get_tokens(code).await?;
    tokens.store(pool).await?;

    Ok(())
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
}
