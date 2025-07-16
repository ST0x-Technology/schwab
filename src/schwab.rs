use base64::prelude::*;
use chrono::{DateTime, Utc};
use clap::Parser;
use reqwest::header::{self, HeaderMap, HeaderValue, InvalidHeaderValue};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;
use thiserror::Error;

pub async fn run_oauth_flow(pool: &SqlitePool, env: &SchwabAuthEnv) -> Result<(), SchwabAuthError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste code (from URL): ");

    let mut code = String::new();
    std::io::stdin().read_line(&mut code).unwrap();
    let code = code.trim();

    let tokens = env.get_tokens(code).await?;
    tokens.store(pool).await?;

    Ok(())
}

#[derive(Parser, Debug)]
pub struct SchwabAuthEnv {
    #[clap(short, long, env)]
    app_key: String,
    #[clap(short, long, env)]
    app_secret: String,
    #[clap(short, long, env, default_value = "https://127.0.0.1")]
    redirect_uri: String,
    #[clap(short, long, env, default_value = "https://api.schwabapi.com")]
    base_url: String,
}

#[derive(Error, Debug)]
pub enum SchwabAuthError {
    #[error("Failed to create header value: {0}")]
    InvalidHeader(#[from] InvalidHeaderValue),
    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

impl SchwabAuthEnv {
    pub fn get_auth_url(&self) -> String {
        format!(
            "{}/v1/oauth/authorize?client_id={}&redirect_uri={}",
            self.base_url, self.app_key, self.redirect_uri
        )
    }

    pub async fn get_tokens(&self, code: &str) -> Result<SchwabTokens, SchwabAuthError> {
        let credentials = format!("{}:{}", self.app_key, self.app_secret);
        let credentials = BASE64_STANDARD.encode(credentials);

        let payload = json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": self.redirect_uri,
        });

        let headers = HeaderMap::from_iter(
            vec![
                (
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Basic {credentials}"))?,
                ),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_str("application/x-www-form-urlencoded")?,
                ),
            ]
            .into_iter(),
        );

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/oauth/token", self.base_url))
            .headers(headers)
            .body(payload.to_string())
            .send()
            .await?;

        let response: SchwabAuthResponse = response.json().await?;

        Ok(SchwabTokens {
            access_token: response.access_token,
            access_token_fetched_at: Utc::now(),
            refresh_token: response.refresh_token,
            refresh_token_fetched_at: Utc::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct SchwabAuthResponse {
    /// Expires every 30 minutes
    access_token: String,
    /// Expires every 7 days
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct SchwabTokens {
    /// Expires every 30 minutes
    access_token: String,
    access_token_fetched_at: DateTime<Utc>,
    /// Expires every 7 days
    refresh_token: String,
    refresh_token_fetched_at: DateTime<Utc>,
}

impl SchwabTokens {
    pub async fn store(&self, pool: &SqlitePool) -> Result<(), SchwabAuthError> {
        sqlx::query!(
            r#"
            INSERT INTO schwab_auth (
                access_token,
                access_token_expires_at,
                refresh_token,
                refresh_token_expires_at
            )
            VALUES (?, ?, ?, ?)
            "#,
            self.access_token,
            self.access_token_fetched_at,
            self.refresh_token,
            self.refresh_token_fetched_at,
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}
