use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use tokio::time::{sleep, timeout};
use url::Url;

use rain_schwab::order::OrderRequestMinimal;
use rain_schwab::schwab_auth::SchwabAuthError;
use rain_schwab::schwab_auth::{AccessToken, SchwabClient};
use rain_schwab::trade::SchwabInstruction;

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
    // Start Prism mock server serving the Trader API
    let mut prism = spawn_prism_mock();

    // Wait until Prism is listening (max 60s)
    const TIMEOUT_SECS: u64 = 60;
    let prism_ready = async {
        let client = reqwest::Client::new();
        let base = "http://127.0.0.1:4020/accounts/accountNumbers";
        loop {
            if client.get(base).send().await.is_ok() {
                break;
            }
            sleep(StdDuration::from_millis(500)).await;
        }
        Ok::<(), SchwabAuthError>(())
    };

    timeout(StdDuration::from_secs(TIMEOUT_SECS), prism_ready)
        .await
        .unwrap()
        .unwrap();

    let api_base = Url::parse("http://127.0.0.1:4020").unwrap();
    // Use dummy access token – Prism ignores authentication headers.
    let dummy_token = AccessToken {
        access_token: "dummy_access_token".into(),
        refresh_token: None,
        expires_at: None,
    };

    let client = SchwabClient::new(api_base, dummy_token);

    // Retrieve account hash
    let hash_value = client.first_account_hash().await?;

    // Preview order endpoint – Prism returns 200
    let order = OrderRequestMinimal::market_equity("AAPL", 1, SchwabInstruction::Buy);
    let preview_payload = serde_json::to_value(&order)?;

    let status = client.preview_order(&hash_value, &preview_payload).await?;
    assert_eq!(status, reqwest::StatusCode::OK);

    let _ = prism.kill();
    Ok(())
}
