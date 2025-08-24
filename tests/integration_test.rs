use clap::Parser;
use httpmock::{Mock, MockServer};
use rain_schwab::env::Env;
use rain_schwab::launch;
use reqwest::Client;
use serde_json::json;
use serial_test::serial;
use std::time::Duration;

/// Creates a test environment with proper mock server configuration
fn create_test_env_with_mock_server(server: &MockServer) -> Env {
    // Use clap to parse from a minimal valid argument set
    let base_url = server.base_url();
    let db_name = ":memory:";

    let args = vec![
        "test",
        "--db",
        db_name,
        "--log-level",
        "info",
        "--ws-rpc-url",
        "ws://127.0.0.1:8545",
        "--orderbook",
        "0x1234567890123456789012345678901234567890",
        "--order-hash",
        "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
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
    let env = create_test_env_with_mock_server(&server);

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
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let client = Client::new();

    // Test 1: Health endpoint should be accessible
    let health_response = client
        .get("http://127.0.0.1:8080/health")
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

    let auth_response = client
        .post("http://127.0.0.1:8080/auth/refresh")
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
        .post("http://127.0.0.1:8080/auth/refresh")
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
