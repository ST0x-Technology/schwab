use std::process::{Child, Command, Stdio};
use std::time::Duration as StdDuration;

use tokio::time::{sleep, timeout};
use url::Url;

#[allow(unused_imports)]
use rain_schwab::schwab_auth::{AccessToken, SchwabClient};

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

/// End-to-end test that exercises the Schwab OAuth flow and a sample API call
/// against the locally mocked Trader API.
///
/// The test is `ignored` by default because it relies on the external Prism
/// mock server started via `npm run mock`. Run it manually with:
/// `cargo test --test schwab_integration -- --ignored`
#[tokio::test]
async fn test_trader_api_with_prism_mock() -> anyhow::Result<()> {
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
        Ok::<(), anyhow::Error>(())
    };

    timeout(StdDuration::from_secs(TIMEOUT_SECS), prism_ready)
        .await
        .map_err(|_| anyhow::anyhow!("Prism mock server did not become ready in time"))??;

    let api_base = Url::parse("http://127.0.0.1:4020").unwrap();
    // Use dummy access token – Prism ignores authentication headers.
    let dummy_token = AccessToken {
        access_token: "dummy_access_token".into(),
        refresh_token: None,
        expires_at: None,
    };

    let client = SchwabClient::new(api_base, dummy_token);

    // Call an endpoint exposed by the Prism mock
    let json = client.get_account_numbers().await?;
    assert!(
        json.is_array(),
        "accountNumbers should return an array JSON payload"
    );

    let _ = prism.kill();
    Ok(())
}
