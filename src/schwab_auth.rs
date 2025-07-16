use base64::{Engine as _, engine::general_purpose};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Module-local Schwab authentication error type.
#[derive(Error, Debug)]
pub enum SchwabAuthError {
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Schwab token endpoint error: {0}")]
    TokenEndpoint(String),
    #[error("no accounts returned")]
    NoAccounts,
    #[error("missing refresh token")]
    MissingRefreshToken,
    #[error("refresh token error: {0}")]
    RefreshEndpoint(String),
}

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

fn basic_auth_header(creds: &SchwabCredentials) -> Option<String> {
    creds.client_secret.as_ref().map(|secret| {
        let pair = format!("{}:{}", creds.client_id, secret);
        let encoded = general_purpose::STANDARD.encode(pair);
        format!("Basic {encoded}")
    })
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
) -> Result<AccessToken, SchwabAuthError> {
    let config = config.cloned().unwrap_or_default();
    let mut token_url = config.auth_base_url.clone();
    token_url.set_path("/v1/oauth/token");

    let client = reqwest::Client::new();

    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", creds.redirect_uri.to_string()),
    ];

    let mut req = client.post(token_url.clone()).form(&params);
    if let Some(basic) = basic_auth_header(creds) {
        req = req.header("Authorization", basic);
    } else {
        // Fallback to public client flow – include client_id in form.
        params.push(("client_id", creds.client_id.clone()));
        req = client.post(token_url).form(&params);
    }

    let res = req.send().await?;

    if !res.status().is_success() {
        let status = res.status();
        let txt = res.text().await.unwrap_or_default();
        return Err(SchwabAuthError::TokenEndpoint(format!(
            "Schwab token endpoint returned {status}: {txt}"
        )));
    }

    let body: TokenEndpointResponse = res.json().await?;

    let expires_at = body
        .expires_in
        .map(|sec| Utc::now() + Duration::seconds(sec));

    Ok(AccessToken {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountNumberHash {
    pub account_number: String,
    pub hash_value: String,
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
    pub async fn get_account_numbers(&self) -> Result<serde_json::Value, SchwabAuthError> {
        let mut url = self.base_url.clone();
        url.set_path("/accounts/accountNumbers");
        let res = self
            .inner
            .get(url)
            .bearer_auth(&self.token.access_token)
            .send()
            .await?;
        let json = res.json::<serde_json::Value>().await?;
        Ok(json)
    }

    /// Returns list of {accountNumber, hashValue}
    pub async fn account_hashes(&self) -> Result<Vec<AccountNumberHash>, SchwabAuthError> {
        let json = self.get_account_numbers().await?;
        let list: Vec<AccountNumberHash> = serde_json::from_value(json)?;
        Ok(list)
    }

    /// Convenience: first hashValue (error if none)
    pub async fn first_account_hash(&self) -> Result<String, SchwabAuthError> {
        let hashes = self.account_hashes().await?;
        hashes
            .get(0)
            .map(|h| h.hash_value.clone())
            .ok_or_else(|| SchwabAuthError::NoAccounts)
    }

    /// Places an order for given account hash with provided JSON payload (already a serde_json::Value).
    pub async fn place_order(
        &self,
        account_hash: &str,
        payload: &serde_json::Value,
    ) -> Result<reqwest::StatusCode, SchwabAuthError> {
        let mut url = self.base_url.clone();
        url.set_path(&format!("/accounts/{}/orders", account_hash));
        let res = self
            .inner
            .post(url)
            .bearer_auth(&self.token.access_token)
            .header("accept", "*/*")
            .json(payload)
            .send()
            .await?;
        Ok(res.status())
    }

    /// Calls POST /accounts/{hash}/previewOrder – returns status
    pub async fn preview_order(
        &self,
        account_hash: &str,
        payload: &serde_json::Value,
    ) -> Result<reqwest::StatusCode, SchwabAuthError> {
        let mut url = self.base_url.clone();
        url.set_path(&format!("/accounts/{account_hash}/previewOrder"));
        let res = self
            .inner
            .post(url)
            .bearer_auth(&self.token.access_token)
            .header("accept", "*/*")
            .json(payload)
            .send()
            .await?;
        Ok(res.status())
    }

    /// Checks token expiration and refreshes if needed (requires refresh_token and creds)
    pub async fn ensure_fresh_token(
        &mut self,
        creds: &SchwabCredentials,
        config: Option<&SchwabAuthConfig>,
    ) -> Result<(), SchwabAuthError> {
        if let Some(exp) = self.token.expires_at {
            if Utc::now() + Duration::seconds(60) < exp {
                return Ok(());
            }
        } else {
            return Ok(());
        }
        let refresh = self
            .token
            .refresh_token
            .clone()
            .ok_or_else(|| SchwabAuthError::MissingRefreshToken)?;
        let new_token = refresh_access_token(creds, &refresh, config).await?;
        self.token = new_token;
        Ok(())
    }
}

/// Performs OAuth refresh_token grant.
pub async fn refresh_access_token(
    creds: &SchwabCredentials,
    refresh_token: &str,
    config: Option<&SchwabAuthConfig>,
) -> Result<AccessToken, SchwabAuthError> {
    let config = config.cloned().unwrap_or_default();
    let mut token_url = config.auth_base_url.clone();
    token_url.set_path("/v1/oauth/token");

    let client = reqwest::Client::new();

    let mut params = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ];

    let mut req = client.post(token_url.clone()).form(&params);
    if let Some(basic) = basic_auth_header(creds) {
        req = req.header("Authorization", basic);
    } else {
        params.push(("client_id", creds.client_id.clone()));
        req = client.post(token_url).form(&params);
    }

    let res = req.send().await?;
    if !res.status().is_success() {
        return Err(SchwabAuthError::RefreshEndpoint(format!(
            "refresh token error: {}",
            res.text().await.unwrap_or_default()
        )));
    }
    let body: TokenEndpointResponse = res.json().await?;
    let expires_at = body.expires_in.map(|s| Utc::now() + Duration::seconds(s));
    Ok(AccessToken {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at,
    })
}
