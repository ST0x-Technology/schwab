use chrono::{DateTime, Utc};
use rocket::serde::json::Json;
use rocket::serde::{Deserialize, Serialize};
use rocket::{Route, State, get, post, routes};
use sqlx::SqlitePool;

use crate::env::Env;
use crate::schwab::extract_code_from_url;

#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: DateTime<Utc>,
}

#[get("/health")]
pub fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now(),
    })
}

#[derive(Deserialize, Serialize)]
pub struct AuthRefreshRequest {
    pub redirect_url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "success")]
pub enum AuthRefreshResponse {
    #[serde(rename = "true")]
    Success { message: String },
    #[serde(rename = "false")]
    Error { error: String },
}

#[post("/auth/refresh", format = "json", data = "<request>")]
pub async fn auth_refresh(
    request: Json<AuthRefreshRequest>,
    pool: &State<SqlitePool>,
    env: &State<Env>,
) -> Json<AuthRefreshResponse> {
    let code = match extract_code_from_url(&request.redirect_url) {
        Ok(code) => code,
        Err(e) => {
            return Json(AuthRefreshResponse::Error {
                error: format!("Failed to extract authorization code: {e}"),
            });
        }
    };

    let tokens = match env.schwab_auth.get_tokens_from_code(&code).await {
        Ok(tokens) => tokens,
        Err(e) => {
            return Json(AuthRefreshResponse::Error {
                error: format!("Authentication failed: {e}"),
            });
        }
    };

    if let Err(e) = tokens.store(pool.inner()).await {
        return Json(AuthRefreshResponse::Error {
            error: format!("Failed to store tokens: {e}"),
        });
    }

    Json(AuthRefreshResponse::Success {
        message: "Authentication successful".to_string(),
    })
}

// Route Configuration
pub fn routes() -> Vec<Route> {
    routes![health, auth_refresh]
}

#[cfg(test)]
mod tests {
    use alloy::primitives::address;
    use httpmock::MockServer;
    use rocket::http::{ContentType, Status};
    use rocket::local::asynchronous::Client;
    use serde_json::json;
    use url::Url;

    use super::*;
    use crate::env::Env;
    use crate::onchain::EvmEnv;
    use crate::schwab::SchwabAuthEnv;
    use crate::test_utils::setup_test_db;

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> Env {
        Env {
            database_url: ":memory:".to_string(),
            log_level: crate::env::LogLevel::Debug,
            schwab_auth: SchwabAuthEnv {
                app_key: "test_app_key".to_string(),
                app_secret: "test_app_secret".to_string(),
                redirect_uri: "https://127.0.0.1".to_string(),
                base_url: mock_server.base_url(),
                account_index: 0,
            },
            evm_env: EvmEnv {
                ws_rpc_url: Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1111111111111111111111111111111111111111"),
                order_owner: address!("0x2222222222222222222222222222222222222222"),
                deployment_block: 0,
            },
            order_polling_interval: 15,
            order_polling_max_jitter: 5,
            dry_run: false,
            otel_metrics_exporter_endpoint: None,
            otel_metrics_exporter_basic_auth_token: None,
            otel_metrics_exporter_instance_id: None,
        }
    }

    #[test]
    fn test_num_of_routes() {
        let routes_list = routes();
        assert_eq!(routes_list.len(), 2);
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let rocket = rocket::build().mount("/", routes![health]);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let response = client.get("/health").dispatch().await;
        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().await.expect("response body");
        let health_response: HealthResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        assert_eq!(health_response.status, "healthy");
        assert!(health_response.timestamp <= chrono::Utc::now());
    }

    #[tokio::test]
    async fn test_auth_refresh_success() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let mock_response = json!({
            "access_token": "new_access_token",
            "refresh_token": "new_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let request_body = json!({
            "redirect_url": "https://127.0.0.1/?code=test_auth_code&state=xyz"
        });

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body(request_body.to_string())
            .dispatch()
            .await;

        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().await.expect("response body");
        let auth_response: AuthRefreshResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        match auth_response {
            AuthRefreshResponse::Success { message } => {
                assert_eq!(message, "Authentication successful");
            }
            AuthRefreshResponse::Error { error } => {
                panic!("Expected success response, got error: {error}")
            }
        }

        mock.assert();
    }

    #[tokio::test]
    async fn test_auth_refresh_invalid_url() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let request_body = json!({
            "redirect_url": "invalid_url"
        });

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body(request_body.to_string())
            .dispatch()
            .await;

        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().await.expect("response body");
        let auth_response: AuthRefreshResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        match auth_response {
            AuthRefreshResponse::Error { error } => {
                assert!(error.contains("Failed to extract authorization code"));
            }
            AuthRefreshResponse::Success { .. } => panic!("Expected error response"),
        }
    }

    #[tokio::test]
    async fn test_auth_refresh_missing_code() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let request_body = json!({
            "redirect_url": "https://127.0.0.1/?state=xyz&other=param"
        });

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body(request_body.to_string())
            .dispatch()
            .await;

        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().await.expect("response body");
        let auth_response: AuthRefreshResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        match auth_response {
            AuthRefreshResponse::Error { error } => {
                assert!(error.contains("Failed to extract authorization code"));
                assert!(error.contains("Missing authorization code parameter"));
            }
            AuthRefreshResponse::Success { .. } => panic!("Expected error response"),
        }
    }

    #[tokio::test]
    async fn test_auth_refresh_schwab_api_error() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/v1/oauth/token");
            then.status(401)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_grant"}));
        });

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let request_body = json!({
            "redirect_url": "https://127.0.0.1/?code=invalid_code&state=xyz"
        });

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body(request_body.to_string())
            .dispatch()
            .await;

        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().await.expect("response body");
        let auth_response: AuthRefreshResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        match auth_response {
            AuthRefreshResponse::Error { error } => {
                assert!(error.contains("Authentication failed"));
            }
            AuthRefreshResponse::Success { .. } => panic!("Expected error response"),
        }

        mock.assert();
    }

    #[tokio::test]
    async fn test_auth_refresh_malformed_json_request() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body("invalid json")
            .dispatch()
            .await;

        // Rocket should return 400 for invalid JSON deserialization
        assert_eq!(response.status(), Status::BadRequest);
    }

    #[tokio::test]
    async fn test_auth_refresh_missing_redirect_url_field() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let rocket = rocket::build()
            .mount("/", routes![auth_refresh])
            .manage(pool)
            .manage(env);
        let client = Client::tracked(rocket)
            .await
            .expect("valid rocket instance");

        let request_body = json!({
            "wrong_field": "https://127.0.0.1/?code=test_code"
        });

        let response = client
            .post("/auth/refresh")
            .header(ContentType::JSON)
            .body(request_body.to_string())
            .dispatch()
            .await;

        // Rocket should return 422 for missing required field
        assert_eq!(response.status(), Status::UnprocessableEntity);
    }
}
