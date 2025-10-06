use backon::{ExponentialBuilder, Retryable};
use clap::Parser;
use httpmock::{Mock, MockServer};
use rain_schwab::env::Env;
use rain_schwab::launch;
use reqwest::Client;
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

/// Creates a test environment with proper mock server configuration
fn create_test_env_with_mock_server(server: &MockServer, server_port: u16) -> Env {
    // Use clap to parse from a minimal valid argument set
    let base_url = server.base_url();
    let db_name = ":memory:";
    let server_port_str = server_port.to_string();

    let args = vec![
        "test",
        "--db",
        db_name,
        "--log-level",
        "info",
        "--server-port",
        &server_port_str,
        "--ws-rpc-url",
        "ws://127.0.0.1:8545",
        "--orderbook",
        "0x1234567890123456789012345678901234567890",
        "--order-owner",
        "0xD2843D9E7738d46D90CB6Dff8D6C83db58B9c165",
        "--deployment-block",
        "1",
        "--app-key",
        "test_app_key",
        "--app-secret",
        "test_app_secret",
        "--redirect-uri",
        "https://127.0.0.1",
        "--base-url",
        &base_url,
        "--account-index",
        "0",
        "--token-encryption-key",
        "0x0000000000000000000000000000000000000000000000000000000000000000",
    ];

    Env::try_parse_from(args).expect("Failed to parse test environment")
}

/// Sets up mock endpoints for Schwab API calls needed during bot operation
fn setup_schwab_api_mocks(server: &MockServer) -> Vec<Mock> {
    vec![
        // Mock for account numbers endpoint (used by bot for validation)
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{"accountNumber": "12345", "hashValue": "hash123"}]));
        }),
        // Mock for token refresh endpoint (used when tokens expire)
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "access_token": "new_access_token",
                    "refresh_token": "new_refresh_token",
                    "expires_in": 1800,
                    "refresh_token_expires_in": 7_776_000,
                    "token_type": "Bearer"
                }));
        }),
    ]
}

#[tokio::test]
#[serial]
async fn test_end_to_end_server_and_bot_integration() {
    let server = MockServer::start();
    let server_port = 8081;
    let env = create_test_env_with_mock_server(&server, server_port);
    let server_base_url = format!("http://127.0.0.1:{server_port}");

    // Set up OAuth mock for authentication testing
    let oauth_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body_contains("grant_type=authorization_code")
            .body_contains("code=test_auth_code");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(json!({
                "access_token": "mock_access_token",
                "refresh_token": "mock_refresh_token",
                "expires_in": 1800,
                "refresh_token_expires_in": 7_776_000,
                "token_type": "Bearer"
            }));
    });

    setup_schwab_api_mocks(&server);

    tokio::spawn(async move { launch(env).await });

    let client = Client::new();
    let health_url = format!("{server_base_url}/health");

    // Wait for server to be ready by polling health endpoint
    let retry_strategy = ExponentialBuilder::default()
        .with_max_delay(Duration::from_secs(1))
        .with_max_times(20); // 20 attempts with exponential backoff

    let health_check = || async { client.get(&health_url).send().await?.error_for_status() };

    health_check
        .retry(&retry_strategy)
        .await
        .expect("Server should become ready within timeout");

    // Test 1: Health endpoint should be accessible
    let health_response = client
        .get(&health_url)
        .send()
        .await
        .expect("Health endpoint should be accessible");

    assert_eq!(health_response.status(), 200);
    let health_data: serde_json::Value = health_response
        .json()
        .await
        .expect("Health response should be valid JSON");
    assert_eq!(health_data["status"], "healthy");
    assert!(health_data["timestamp"].is_string());

    // Test 2: Manual authentication flow through API
    let auth_request = json!({
        "redirect_url": "https://127.0.0.1?code=test_auth_code&session=session123"
    });

    let auth_url = format!("{server_base_url}/auth/refresh");
    let auth_response = client
        .post(&auth_url)
        .json(&auth_request)
        .send()
        .await
        .expect("Auth endpoint should be accessible");

    assert_eq!(auth_response.status(), 200);
    let auth_data: serde_json::Value = auth_response
        .json()
        .await
        .expect("Auth response should be valid JSON");

    assert_eq!(auth_data["success"], "true");
    assert!(
        auth_data["message"]
            .as_str()
            .unwrap()
            .contains("Authentication successful")
    );

    // Verify OAuth endpoint was called
    oauth_mock.assert();

    // Test 3: Error handling - test auth endpoint with invalid URL
    let invalid_auth_request = json!({
        "redirect_url": "https://127.0.0.1?error=access_denied"
    });

    let error_response = client
        .post(&auth_url)
        .json(&invalid_auth_request)
        .send()
        .await
        .expect("Auth endpoint should handle errors");

    assert_eq!(error_response.status(), 200);
    let error_data: serde_json::Value = error_response
        .json()
        .await
        .expect("Error response should be valid JSON");

    assert_eq!(error_data["success"], "false");
}
