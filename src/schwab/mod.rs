use crate::error;
use reqwest::header::InvalidHeaderValue;
use sqlx::SqlitePool;
use std::io::{self, Write};
use thiserror::Error;

pub(crate) mod auth;
pub(crate) mod execution;
pub(crate) mod market_hours;
pub(crate) mod market_hours_cache;
pub(crate) mod order;
pub(crate) mod order_poller;
pub(crate) mod order_status;
pub(crate) mod tokens;
pub(crate) mod trade_state;

pub(crate) use auth::SchwabAuthEnv;
pub(crate) use order_poller::{OrderPollerConfig, OrderStatusPoller};
pub(crate) use tokens::SchwabTokens;
pub(crate) use trade_state::{HasTradeStatus, TradeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeStatus {
    Pending,
    Submitted,
    Filled,
    Failed,
}

impl TradeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Submitted => "SUBMITTED",
            Self::Filled => "FILLED",
            Self::Failed => "FAILED",
        }
    }
}

impl std::str::FromStr for TradeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PENDING" => Ok(Self::Pending),
            "SUBMITTED" => Ok(Self::Submitted),
            "FILLED" => Ok(Self::Filled),
            "FAILED" => Ok(Self::Failed),
            _ => Err(format!("Invalid trade status: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Buy,
    Sell,
}

impl Direction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

impl std::str::FromStr for Direction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            _ => Err(format!("Invalid direction: {s}")),
        }
    }
}

impl serde::Serialize for Direction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Error, Debug)]
pub(crate) enum SchwabError {
    #[error("Failed to create header value: {0}")]
    InvalidHeader(#[from] InvalidHeaderValue),
    #[error("Request failed: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("URL parsing failed: {0}")]
    Url(#[from] url::ParseError),
    #[error("Missing authorization code parameter in URL: {url}")]
    MissingAuthCode { url: String },
    #[error("JSON serialization failed: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("Refresh token has expired")]
    RefreshTokenExpired,
    #[error("No accounts found")]
    NoAccountsFound,
    #[error("Account index {index} out of bounds (found {count} accounts)")]
    AccountIndexOutOfBounds { index: usize, count: usize },
    #[error("{action} failed with status: {status}, body: {body}")]
    RequestFailed {
        action: String,
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("Execution persistence error: {0}")]
    ExecutionPersistence(#[from] crate::error::PersistenceError),
}

pub(crate) async fn run_oauth_flow(
    pool: &SqlitePool,
    env: &SchwabAuthEnv,
) -> Result<(), SchwabError> {
    println!(
        "Authenticate portfolio brokerage account (not dev account) and paste URL: {}",
        env.get_auth_url()
    );
    print!("Paste the full redirect URL you were sent to: ");
    io::stdout().flush()?;

    let mut redirect_url = String::new();
    io::stdin().read_line(&mut redirect_url)?;
    let redirect_url = redirect_url.trim();

    let code = extract_code_from_url(redirect_url)?;
    println!("Extracted code: {code}");

    let tokens = env.get_tokens_from_code(&code).await?;
    tokens.store(pool).await?;

    Ok(())
}

pub(crate) const fn shares_from_db_i64(db_value: i64) -> Result<u64, error::OnChainError> {
    if db_value < 0 {
        Err(error::OnChainError::Persistence(
            error::PersistenceError::InvalidShareQuantity(db_value),
        ))
    } else {
        #[allow(clippy::cast_sign_loss)]
        Ok(db_value as u64)
    }
}

pub(crate) const fn price_cents_from_db_i64(db_value: i64) -> Result<u64, error::OnChainError> {
    if db_value < 0 {
        Err(error::OnChainError::Persistence(
            error::PersistenceError::InvalidPriceCents(db_value),
        ))
    } else {
        #[allow(clippy::cast_sign_loss)]
        Ok(db_value as u64)
    }
}

pub(crate) fn extract_code_from_url(url: &str) -> Result<String, SchwabError> {
    let parsed_url = url::Url::parse(url)?;

    parsed_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| SchwabError::MissingAuthCode {
            url: url.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::setup_test_db;
    use httpmock::prelude::*;
    use serde_json::json;

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
            account_index: 0,
        }
    }

    #[tokio::test]
    async fn test_run_oauth_flow() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;

        let mock_response = json!({
            "access_token": "test_access_token",
            "refresh_token": "test_refresh_token"
        });

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=authorization_code")
                .body_contains("code=test_code")
                .body_contains("redirect_uri=https%3A%2F%2F127.0.0.1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(mock_response);
        });

        let tokens = env.get_tokens_from_code("test_code").await.unwrap();
        tokens.store(&pool).await.unwrap();

        mock.assert();
    }

    #[test]
    fn test_extract_code_from_url_success() {
        let url = "https://127.0.0.1/?code=test_auth_code&state=xyz";
        assert_eq!(extract_code_from_url(url).unwrap(), "test_auth_code");
    }

    #[test]
    fn test_extract_code_from_url_missing_code() {
        let url = "https://127.0.0.1/?state=xyz&other=param";
        let result = extract_code_from_url(url);
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::MissingAuthCode { url: ref u } if u == "https://127.0.0.1/?state=xyz&other=param"
        ));
    }

    #[test]
    fn test_extract_code_from_url_invalid_url() {
        let url = "not_a_valid_url";
        assert!(matches!(
            extract_code_from_url(url).unwrap_err(),
            SchwabError::Url(_)
        ));
    }

    #[test]
    fn test_extract_code_from_url_no_query_params() {
        let url = "https://127.0.0.1/";
        let result = extract_code_from_url(url);
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::MissingAuthCode { url: ref u } if u == "https://127.0.0.1/"
        ));
    }

    #[test]
    fn test_shares_from_db_i64_positive() {
        assert_eq!(shares_from_db_i64(100).unwrap(), 100);
        assert_eq!(shares_from_db_i64(0).unwrap(), 0);
        assert_eq!(shares_from_db_i64(i64::MAX).unwrap(), i64::MAX as u64);
    }

    #[test]
    fn test_shares_from_db_i64_negative() {
        shares_from_db_i64(-1).unwrap_err();
        shares_from_db_i64(-100).unwrap_err();
        shares_from_db_i64(i64::MIN).unwrap_err();
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_http_401() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header(
                    "authorization",
                    "Basic dGVzdF9hcHBfa2V5OnRlc3RfYXBwX3NlY3JldA==",
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=authorization_code")
                .body_contains("code=invalid_code");
            then.status(401)
                .header("content-type", "application/json")
                .json_body(json!({"error": "invalid_grant"}));
        });

        let result = env.get_tokens_from_code("invalid_code").await;
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "get tokens" && status.as_u16() == 401
        ));

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_http_500() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(500);
        });

        let result = env.get_tokens_from_code("test_code").await;
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "get tokens" && status.as_u16() == 500
        ));

        mock.assert();
    }

    #[tokio::test]
    async fn test_get_tokens_from_code_invalid_json_response() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/oauth/token");
            then.status(200)
                .header("content-type", "application/json")
                .body("invalid json");
        });

        assert!(matches!(
            env.get_tokens_from_code("test_code").await.unwrap_err(),
            SchwabError::Reqwest(_)
        ));

        mock.assert();
    }

    #[test]
    fn test_schwab_instruction_as_str() {
        assert_eq!(Direction::Buy.as_str(), "BUY");
        assert_eq!(Direction::Sell.as_str(), "SELL");
    }

    #[test]
    fn test_schwab_instruction_from_str() {
        assert_eq!("BUY".parse::<Direction>().unwrap(), Direction::Buy);
        assert_eq!("SELL".parse::<Direction>().unwrap(), Direction::Sell);

        let result = "INVALID".parse::<Direction>();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Invalid direction: INVALID");
    }

    #[test]
    fn test_schwab_instruction_serialize() {
        let buy_json = serde_json::to_string(&Direction::Buy).unwrap();
        assert_eq!(buy_json, "\"BUY\"");

        let sell_json = serde_json::to_string(&Direction::Sell).unwrap();
        assert_eq!(sell_json, "\"SELL\"");
    }
}
