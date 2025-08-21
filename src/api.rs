use chrono::{DateTime, Utc};
use rocket::serde::json::Json;
use rocket::serde::{Deserialize, Serialize};
use rocket::{Route, State, get, post, routes};
use sqlx::SqlitePool;

use crate::env::Env;
use crate::schwab::extract_code_from_url;

#[derive(Serialize)]
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
pub struct AuthRefreshResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error: Option<String>,
}

#[post("/auth/refresh", format = "json", data = "<request>")]
pub async fn auth_refresh(
    request: Json<AuthRefreshRequest>,
    pool: &State<SqlitePool>,
    env: &State<Env>,
) -> Result<Json<AuthRefreshResponse>, Json<AuthRefreshResponse>> {
    let code = match extract_code_from_url(&request.redirect_url) {
        Ok(code) => code,
        Err(e) => {
            return Err(Json(AuthRefreshResponse {
                success: false,
                message: None,
                error: Some(format!("Failed to extract authorization code: {e}")),
            }));
        }
    };

    let tokens = match env.schwab_auth.get_tokens_from_code(&code).await {
        Ok(tokens) => tokens,
        Err(e) => {
            return Err(Json(AuthRefreshResponse {
                success: false,
                message: None,
                error: Some(format!("Authentication failed: {e}")),
            }));
        }
    };

    if let Err(e) = tokens.store(pool.inner()).await {
        return Err(Json(AuthRefreshResponse {
            success: false,
            message: None,
            error: Some(format!("Failed to store tokens: {e}")),
        }));
    }

    Ok(Json(AuthRefreshResponse {
        success: true,
        message: Some("Authentication successful".to_string()),
        error: None,
    }))
}

// Route Configuration
pub fn routes() -> Vec<Route> {
    routes![health, auth_refresh]
}
