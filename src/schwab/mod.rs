pub mod auth;
pub mod tokens;

pub use auth::{SchwabAuthEnv, SchwabAuthError, SchwabAuthResponse};
pub use tokens::SchwabTokens;

use sqlx::SqlitePool;
use std::io;

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabAuthError> {
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
