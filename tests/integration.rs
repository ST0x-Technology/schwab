use std::process::{Child, Command, Stdio};
use tokio::time::{Duration, sleep, timeout};

use rain_schwab::schwab::SchwabAuthError;

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
    let mut prism = spawn_prism_mock();

    const TIMEOUT_SECS: u64 = 60;
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

    Ok(())
}
