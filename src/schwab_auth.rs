use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

/// Holds OAuth access token and associated metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>, // calculated from `expires_in`
}

#[derive(Debug, Clone)]
pub struct SchwabCredentials {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: Url,
}

#[derive(Debug, Clone)]
pub struct SchwabAuthConfig {
    /// Base URL for OAuth endpoints (defaults to Schwab production)
    pub auth_base_url: Url,
}

impl Default for SchwabAuthConfig {
    fn default() -> Self {
        Self {
            auth_base_url: Url::parse("https://api.schwabapi.com").expect("hard-coded url"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    token_type: Option<String>,
}

/// Exchanges an authorization `code` for an access token using Schwab OAuth2.
///
/// This helper is written directly against the Schwab API so that we avoid
/// pulling in a full-blown OAuth2 library. It is also test-friendly – the
/// `config` parameter allows overriding the host so that the interaction can
/// be mocked.
pub async fn exchange_code_for_token(
    creds: &SchwabCredentials,
    code: &str,
    config: Option<&SchwabAuthConfig>,
) -> anyhow::Result<AccessToken> {
    let config = config.cloned().unwrap_or_default();
    let mut token_url = config.auth_base_url.clone();
    token_url.set_path("/v1/oauth/token");

    let client = reqwest::Client::new();

    // Schwab expects x-www-form-urlencoded
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("client_id", creds.client_id.clone()),
        ("redirect_uri", creds.redirect_uri.to_string()),
    ];
    if let Some(secret) = &creds.client_secret {
        params.push(("client_secret", secret.clone()));
    }

    let res = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .context("failed to send token request")?;

    if !res.status().is_success() {
        let status = res.status();
        let txt = res.text().await.unwrap_or_default();
        anyhow::bail!("Schwab token endpoint returned {status}: {txt}");
    }

    let body: TokenEndpointResponse = res.json().await.context("invalid token JSON")?;

    let expires_at = body
        .expires_in
        .map(|sec| Utc::now() + Duration::seconds(sec));

    Ok(AccessToken {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at,
    })
}

/// Simple API client wrapper that automatically adds the `Authorization`
/// header when making requests against the Trader API.
#[derive(Debug, Clone)]
pub struct SchwabClient {
    inner: reqwest::Client,
    base_url: Url,
    token: AccessToken,
}

impl SchwabClient {
    pub fn new(base_url: Url, token: AccessToken) -> Self {
        Self {
            inner: reqwest::Client::new(),
            base_url,
            token,
        }
    }

    /// GET /accounts/accountNumbers – convenience wrapper used in tests
    pub async fn get_account_numbers(&self) -> anyhow::Result<serde_json::Value> {
        let mut url = self.base_url.clone();
        url.set_path("/accounts/accountNumbers");
        let res = self
            .inner
            .get(url)
            .bearer_auth(&self.token.access_token)
            .send()
            .await
            .context("failed sending request")?;
        let json = res.json::<serde_json::Value>().await?;
        Ok(json)
    }
}
