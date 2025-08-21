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

#[derive(Deserialize)]
pub struct AuthRefreshRequest {
    pub redirect_url: String,
}

#[derive(Serialize)]
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
    use super::*;
    use rocket::http::Status;
    use rocket::local::blocking::Client;

    #[test]
    fn test_health_endpoint() {
        let rocket = rocket::build().mount("/", routes![health]);
        let client = Client::tracked(rocket).expect("valid rocket instance");

        let response = client.get("/health").dispatch();
        assert_eq!(response.status(), Status::Ok);

        let body = response.into_string().expect("response body");
        let health_response: HealthResponse =
            serde_json::from_str(&body).expect("valid JSON response");

        assert_eq!(health_response.status, "healthy");
        assert!(health_response.timestamp <= chrono::Utc::now());
    }
}
