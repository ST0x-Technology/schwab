use std::process::{Child, Command, Stdio};
use std::thread;
use tokio::time::{Duration, sleep, timeout};

use chrono::{Duration as ChronoDuration, Utc};
use httpmock::prelude::*;
use rain_schwab::schwab::{SchwabAuthEnv, SchwabAuthError, SchwabTokens};
use sqlx::SqlitePool;

/// Spawn the Prism mock server defined in `package.json` (OpenAPI based mocks).
/// Requires Node and `npm install` to have been run.
fn spawn_prism_mock() -> Child {
    Command::new("npm")
        .arg("run")
        .arg("--silent")
        .arg("mock")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start prism mock server; ensure Node and prism are installed")
}

#[tokio::test]
async fn test_trader_api_with_prism_mock() -> Result<(), SchwabAuthError> {
    const TIMEOUT_SECS: u64 = 60;
    let mut prism = spawn_prism_mock();
    let prism_ready = async {
        let client = reqwest::Client::new();
        let base = "http://127.0.0.1:4020/accounts/accountNumbers";
        loop {
            if client.get(base).send().await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(500)).await;
        }
        Ok::<(), SchwabAuthError>(())
    };

    timeout(Duration::from_secs(TIMEOUT_SECS), prism_ready)
        .await
        .unwrap()
        .unwrap();

    let _ = prism.kill();
    let _ = prism.wait();

    Ok(())
}

async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
    SchwabAuthEnv {
        app_key: "test_app_key".to_string(),
        app_secret: "test_app_secret".to_string(),
        redirect_uri: "https://127.0.0.1".to_string(),
        base_url: mock_server.base_url(),
    }
}

#[tokio::test]
async fn test_automatic_token_refresh_before_expiration() -> Result<(), SchwabAuthError> {
    let server = MockServer::start();
    let env = create_test_env_with_mock_server(&server);
    let pool = setup_test_db().await;
    let now = Utc::now();

    let tokens = SchwabTokens {
        access_token: "near_expiration_access_token".to_string(),
        access_token_fetched_at: now - ChronoDuration::minutes(29),
        refresh_token: "valid_refresh_token".to_string(),
        refresh_token_fetched_at: now - ChronoDuration::days(1),
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
                Duration::from_secs(5),
                SchwabTokens::start_automatic_token_refresh(pool_clone, env_clone)
            ).await
        })
    });

    sleep(Duration::from_millis(2000)).await;

    handle.join().unwrap().unwrap_err();

    mock.assert();

    let stored_tokens = SchwabTokens::load(&pool).await?;
    assert_eq!(stored_tokens.access_token, "refreshed_access_token");
    assert_eq!(stored_tokens.refresh_token, "new_refresh_token");

    Ok(())
}
