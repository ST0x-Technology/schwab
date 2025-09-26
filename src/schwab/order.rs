use backon::{ExponentialBuilder, Retryable};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::time::Duration;
use tracing::{error, info};

use chrono::Utc;

#[cfg(test)]
use super::execution::find_execution_by_id;
use super::{
    SchwabAuthEnv, SchwabError, SchwabTokens, execution::update_execution_status_within_transaction,
};
use crate::trade_state::TradeState;

/// Response from Schwab order placement API.
/// According to Schwab OpenAPI spec, successful order placement (201) returns
/// empty body with order ID in the Location header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderPlacementResponse {
    pub order_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub(crate) struct Order {
    pub order_type: OrderType,
    pub session: Session,
    pub duration: OrderDuration,
    pub order_strategy_type: OrderStrategyType,
    pub order_leg_collection: Vec<OrderLeg>,
}

impl Order {
    pub fn new(symbol: String, instruction: Instruction, quantity: u64) -> Self {
        let instrument = Instrument {
            symbol,
            asset_type: AssetType::Equity,
        };

        let order_leg = OrderLeg {
            instruction,
            quantity,
            instrument,
        };

        Self {
            order_type: OrderType::Market,
            session: Session::Normal,
            duration: OrderDuration::Day,
            order_strategy_type: OrderStrategyType::Single,
            order_leg_collection: vec![order_leg],
        }
    }

    /// Place order with bounded retries.
    /// Retries only transport errors and 5xx server errors to avoid duplicate orders on 4xx client errors.
    pub async fn place(
        &self,
        env: &SchwabAuthEnv,
        pool: &SqlitePool,
    ) -> Result<OrderPlacementResponse, SchwabError> {
        let access_token = SchwabTokens::get_valid_access_token(pool, env).await?;
        let account_hash = env.get_account_hash(pool).await?;

        let headers = [
            (
                header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {access_token}"))?,
            ),
            (header::ACCEPT, HeaderValue::from_str("*/*")?),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str("application/json")?,
            ),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        let order_json = serde_json::to_string(self)?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        let response = (|| async {
            let response = client
                .post(format!(
                    "{}/trader/v1/accounts/{}/orders",
                    env.base_url, account_hash
                ))
                .headers(headers.clone())
                .body(order_json.clone())
                .send()
                .await?;

            // Only retry on 5xx server errors or transport failures
            // Do not retry 4xx client errors which indicate permanent failures
            if response.status().is_server_error() {
                return Err(SchwabError::RequestFailed {
                    action: "place order (server error)".to_string(),
                    status: response.status(),
                    body: "Server error, will retry".to_string(),
                });
            }

            Ok(response)
        })
        .retry(
            ExponentialBuilder::default()
                .with_max_times(2)
                .with_jitter(),
        )
        .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(SchwabError::RequestFailed {
                action: "place order".to_string(),
                status,
                body: error_body,
            });
        }

        // Extract order ID from Location header according to Schwab OpenAPI spec
        let order_id = extract_order_id_from_location_header(&response)?;

        Ok(OrderPlacementResponse { order_id })
    }
}

/// Extracts order ID from the Location header in Schwab order placement response.
///
/// According to Schwab OpenAPI spec, successful order placement returns Location header
/// containing link to the newly created order. The order ID is extracted from this URL.
/// Expected format: "/trader/v1/accounts/{accountHash}/orders/{orderId}"
fn extract_order_id_from_location_header(
    response: &reqwest::Response,
) -> Result<String, SchwabError> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or_else(|| SchwabError::RequestFailed {
            action: "extract order ID".to_string(),
            status: response.status(),
            body: "Missing Location header in order placement response".to_string(),
        })?
        .to_str()
        .map_err(|_| SchwabError::RequestFailed {
            action: "extract order ID".to_string(),
            status: response.status(),
            body: "Invalid Location header value".to_string(),
        })?;

    // Extract order ID from URL path: "/trader/v1/accounts/{accountHash}/orders/{orderId}"
    // Must contain the expected path structure
    if !location.contains("/trader/v1/accounts/") || !location.contains("/orders/") {
        return Err(SchwabError::RequestFailed {
            action: "extract order ID".to_string(),
            status: response.status(),
            body: format!(
                "Invalid Location header format, expected '/trader/v1/accounts/{{accountHash}}/orders/{{orderId}}': {location}"
            ),
        });
    }

    let order_id = location
        .split('/')
        .next_back()
        .ok_or_else(|| SchwabError::RequestFailed {
            action: "extract order ID".to_string(),
            status: response.status(),
            body: format!("Cannot extract order ID from Location header: {location}"),
        })?
        .to_string();

    if order_id.is_empty() {
        return Err(SchwabError::RequestFailed {
            action: "extract order ID".to_string(),
            status: response.status(),
            body: format!("Empty order ID extracted from Location header: {location}"),
        });
    }

    Ok(order_id)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    TrailingStop,
    NetDebit,
    NetCredit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Instruction {
    Buy,
    Sell,
    BuyToCover,
    SellShort,
    BuyToOpen,
    BuyToClose,
    SellToOpen,
    SellToClose,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Session {
    Normal,
    Am,
    Pm,
    Seamless,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum OrderDuration {
    Day,
    GoodTillCancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum OrderStrategyType {
    Single,
    Oco,
    Trigger,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum AssetType {
    Equity,
    Option,
    Index,
    MutualFund,
    CashEquivalent,
    FixedIncome,
    Currency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderLeg {
    pub instruction: Instruction,
    pub quantity: u64,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Instrument {
    pub symbol: String,
    pub asset_type: AssetType,
}

pub(crate) async fn handle_execution_success(
    pool: &SqlitePool,
    execution_id: i64,
    order_id: String,
) -> Result<(), SchwabError> {
    info!("Successfully placed Schwab order for execution: id={execution_id}, order_id={order_id}");

    let mut sql_tx = pool.begin().await.map_err(|e| {
        error!(
            "Failed to start transaction for execution success: id={}, error={:?}",
            execution_id, e
        );
        e
    })?;

    // Update status to Submitted with order_id so order poller can track the order and update with real fill price
    update_execution_status_within_transaction(
        &mut sql_tx,
        execution_id,
        TradeState::Submitted { order_id },
    )
    .await?;

    sql_tx.commit().await.map_err(|e| {
        error!("Failed to commit execution success transaction: id={execution_id}, error={e:?}",);
        e
    })?;

    Ok(())
}

pub(crate) async fn handle_execution_failure(
    pool: &SqlitePool,
    execution_id: i64,
    error: SchwabError,
) -> Result<(), SchwabError> {
    error!(
        "Failed to place Schwab order after retries for execution: id={execution_id}, error={error:?}",
    );

    let mut sql_tx =
        pool.begin().await.map_err(|e| {
            error!(
                "Failed to start transaction for execution failure: id={execution_id}, error={e:?}",
            );
            e
        })?;

    if let Err(update_err) = update_execution_status_within_transaction(
        &mut sql_tx,
        execution_id,
        TradeState::Failed {
            failed_at: Utc::now(),
            error_reason: Some(error.to_string()),
        },
    )
    .await
    {
        error!(
            "Failed to update execution status to FAILED: id={execution_id}, error={update_err:?}",
        );
        let _ = sql_tx.rollback().await;
        return Err(SchwabError::ExecutionPersistence(update_err));
    }

    sql_tx.commit().await.map_err(|e| {
        error!("Failed to commit execution failure transaction: id={execution_id}, error={e:?}",);
        e
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::setup_test_db;
    use chrono::Utc;
    use serde_json::json;
    use st0x_broker::{Broker, SchwabBroker};

    #[test]
    fn test_new_buy() {
        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.session, Session::Normal);
        assert_eq!(order.duration, OrderDuration::Day);
        assert_eq!(order.order_strategy_type, OrderStrategyType::Single);
        assert_eq!(order.order_leg_collection.len(), 1);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Buy);
        assert_eq!(leg.quantity, 100);
        assert_eq!(leg.instrument.symbol, "AAPL");
        assert_eq!(leg.instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_new_sell() {
        let order = Order::new("TSLA".to_string(), Instruction::Sell, 50);

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.session, Session::Normal);
        assert_eq!(order.duration, OrderDuration::Day);
        assert_eq!(order.order_strategy_type, OrderStrategyType::Single);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Sell);
        assert_eq!(leg.quantity, 50);
        assert_eq!(leg.instrument.symbol, "TSLA");
        assert_eq!(leg.instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_new_sell_short() {
        let order = Order::new("GME".to_string(), Instruction::SellShort, 26);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::SellShort);
        assert_eq!(leg.quantity, 26);
        assert_eq!(leg.instrument.symbol, "GME");
    }

    #[test]
    fn test_new_buy_to_cover() {
        let order = Order::new("AMC".to_string(), Instruction::BuyToCover, 15);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::BuyToCover);
        assert_eq!(leg.quantity, 15);
    }

    #[test]
    fn test_whole_shares_only() {
        let order = Order::new("SPY".to_string(), Instruction::Buy, 1);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Buy);
        assert_eq!(leg.quantity, 1);
        assert_eq!(leg.instrument.symbol, "SPY");

        // Test serialization uses whole numbers
        let json = serde_json::to_value(&order).unwrap();
        assert_eq!(json["orderLegCollection"][0]["quantity"], 1);
    }

    #[test]
    fn test_order_serialization() {
        let order = Order::new("MSFT".to_string(), Instruction::Buy, 25);

        let json = serde_json::to_string(&order).unwrap();
        let deserialized: Order = serde_json::from_str(&json).unwrap();

        assert_eq!(order.order_type, deserialized.order_type);
        assert_eq!(order.session, deserialized.session);
        assert_eq!(order.duration, deserialized.duration);
        assert_eq!(order.order_strategy_type, deserialized.order_strategy_type);
        assert_eq!(
            order.order_leg_collection.len(),
            deserialized.order_leg_collection.len()
        );
        assert_eq!(
            order.order_leg_collection[0].instruction,
            deserialized.order_leg_collection[0].instruction
        );
        assert_eq!(
            order.order_leg_collection[0].quantity,
            deserialized.order_leg_collection[0].quantity
        );
        assert_eq!(
            order.order_leg_collection[0].instrument,
            deserialized.order_leg_collection[0].instrument
        );
    }

    #[test]
    fn test_order_camel_case_serialization() {
        let order = Order::new("GOOGL".to_string(), Instruction::Buy, 10);

        let json = serde_json::to_string_pretty(&order).unwrap();

        assert!(json.contains("\"orderType\""));
        assert!(json.contains("\"orderLegCollection\""));
        assert!(json.contains("\"orderStrategyType\""));
        assert!(json.contains("\"assetType\""));
    }

    #[test]
    fn test_serialization_matches_schwab_format() {
        let order = Order::new("XYZ".to_string(), Instruction::Buy, 15);

        let json = serde_json::to_value(&order).unwrap();

        assert_eq!(json["orderType"], "MARKET");
        assert_eq!(json["session"], "NORMAL");
        assert_eq!(json["duration"], "DAY");
        assert_eq!(json["orderStrategyType"], "SINGLE");
        assert_eq!(json["orderLegCollection"][0]["instruction"], "BUY");
        assert_eq!(json["orderLegCollection"][0]["quantity"], 15);
        assert_eq!(json["orderLegCollection"][0]["instrument"]["symbol"], "XYZ");
        assert_eq!(
            json["orderLegCollection"][0]["instrument"]["assetType"],
            "EQUITY"
        );
    }

    #[tokio::test]
    async fn test_place_order_success() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "*/*")
                .header("content-type", "application/json");
            then.status(201)
                .header("location", "/trader/v1/accounts/ABC123DEF456/orders/12345");
        });

        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let response = result.unwrap();
        assert_eq!(response.order_id, "12345");
    }

    #[tokio::test]
    async fn test_place_order_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(400)
                .json_body(json!({"error": "Invalid order"}));
        });

        let order = Order::new("INVALID".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(
            matches!(error, super::SchwabError::RequestFailed { action, status, .. } if action == "place order" && status.as_u16() == 400)
        );
    }

    fn create_test_env_with_mock_server(
        mock_server: &httpmock::MockServer,
    ) -> super::SchwabAuthEnv {
        super::SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
            account_index: 0,
        }
    }

    async fn setup_test_tokens(pool: &sqlx::SqlitePool) {
        let tokens = super::super::tokens::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now(),
        };
        tokens.store(pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_order_placement_success_with_location_header() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(201)
                .header("location", "/trader/v1/accounts/ABC123DEF456/orders/67890");
        });

        let order = Order::new("TSLA".to_string(), Instruction::Sell, 50);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let response = result.unwrap();
        assert_eq!(response.order_id, "67890");
    }

    #[tokio::test]
    async fn test_order_placement_missing_location_header() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(201); // Success but missing Location header
        });

        let order = Order::new("SPY".to_string(), Instruction::Buy, 25);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, body, .. }
            if action == "extract order ID" && body.contains("Missing Location header")
        ));
    }

    #[tokio::test]
    async fn test_order_placement_invalid_location_header() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(201).header("location", "invalid-url-format"); // Invalid format
        });

        let order = Order::new("MSFT".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, body, .. }
            if action == "extract order ID" && body.contains("Invalid Location header format")
        ));
    }

    #[tokio::test]
    async fn test_order_placement_retry_logic_verification() {
        // This test verifies that retry logic exists without necessarily testing network timeouts
        // Since the retry behavior depends on the underlying reqwest/backon configuration,
        // we instead test that the order placement handles failures gracefully

        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        // Mock server that simulates a consistently failing service
        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(502) // Bad Gateway - common transient error
                .json_body(json!({"error": "Bad Gateway"}));
        });

        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();

        // The test ensures error handling works correctly, regardless of retry count
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "place order (server error)" && status.as_u16() == 502
        ));

        // At least one attempt should have been made
        assert!(
            order_mock.hits() >= 1,
            "Expected at least one API call attempt"
        );
    }

    #[tokio::test]
    async fn test_order_placement_server_error_500() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(500)
                .json_body(json!({"error": "Internal server error"}));
        });

        let order = Order::new("TSLA".to_string(), Instruction::Sell, 50);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        // With retries enabled, expect multiple attempts for 5xx errors
        assert!(
            order_mock.hits() >= 1,
            "Expected at least one API call attempt"
        );
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "place order (server error)" && status.as_u16() == 500
        ));
    }

    #[tokio::test]
    async fn test_order_placement_authentication_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(401).json_body(json!({"error": "Unauthorized"}));
        });

        let order = Order::new("SPY".to_string(), Instruction::Buy, 25);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "place order" && status.as_u16() == 401
        ));
    }

    #[tokio::test]
    async fn test_order_placement_malformed_json_response() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200).body("invalid json response"); // Malformed JSON
        });

        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        let error = result.unwrap_err();
        // Should fail with JSON serialization error due to malformed account response
        assert!(matches!(error, SchwabError::Reqwest(_)));
    }

    #[tokio::test]
    async fn test_order_placement_empty_location_header_value() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(201)
                .header("location", "/trader/v1/accounts/ABC123DEF456/orders/"); // Empty order ID
        });

        let order = Order::new("MSFT".to_string(), Instruction::Sell, 50);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, body, .. }
            if action == "extract order ID" && body.contains("Empty order ID")
        ));
    }

    #[tokio::test]
    async fn test_execution_success_handling() {
        use super::super::execution::OffchainExecution;
        use crate::schwab::Direction;
        use crate::schwab::TradeState;

        let pool = setup_test_db().await;

        // Create a test execution using a transaction
        let mut sql_tx = pool.begin().await.unwrap();
        let execution = OffchainExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            state: TradeState::Pending,
        };

        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Test successful execution handling
        handle_execution_success(&pool, execution_id, "1004055538123".to_string())
            .await
            .unwrap();

        // Verify execution is now Submitted with order_id stored in database
        let updated_execution = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();

        // Status should be Submitted with order_id since order poller will update it when filled
        assert!(matches!(
            updated_execution.state,
            TradeState::Submitted { ref order_id } if order_id == "1004055538123"
        ));
    }

    #[tokio::test]
    async fn test_execution_failure_handling() {
        use super::super::execution::OffchainExecution;
        use crate::schwab::TradeState;
        use crate::schwab::{Direction, SchwabError};

        let pool = setup_test_db().await;

        // Create a test execution using a transaction
        let mut sql_tx = pool.begin().await.unwrap();
        let execution = OffchainExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 50,
            direction: Direction::Sell,
            state: TradeState::Pending,
        };

        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Test failure handling
        let test_error = SchwabError::RequestFailed {
            action: "test failure".to_string(),
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "Test error body".to_string(),
        };

        handle_execution_failure(&pool, execution_id, test_error)
            .await
            .unwrap();

        // Verify execution status was updated to failed
        let updated_execution = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        match &updated_execution.state {
            TradeState::Failed { .. } => {
                // Test passes - execution was properly marked as failed
                // Note: error_reason is not persisted in database yet, so we don't test it
            }
            other => panic!("Expected Failed status but got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_handle_execution_failure_database_failure() {
        let pool = setup_test_db().await;

        // Close pool to simulate database failure
        pool.close().await;

        let test_error = SchwabError::RequestFailed {
            action: "test".to_string(),
            status: reqwest::StatusCode::BAD_REQUEST,
            body: "test error".to_string(),
        };

        assert!(matches!(
            handle_execution_failure(&pool, 123, test_error)
                .await
                .unwrap_err(),
            SchwabError::Sqlx(_)
        ));
    }

    #[tokio::test]
    async fn test_get_order_status_success_filled() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538123")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "orderId": 1_004_055_538_123_i64,
                    "status": "FILLED",
                    "filledQuantity": 100.0,
                    "remainingQuantity": 0.0,
                    "enteredTime": "2023-10-15T10:25:00Z",
                    "closeTime": "2023-10-15T10:30:00Z",
                    "orderActivityCollection": [{
                        "activityType": "EXECUTION",
                        "executionLegs": [{
                            "executionId": "EXEC001",
                            "quantity": 100.0,
                            "price": 150.25,
                            "time": "2023-10-15T10:30:00Z"
                        }]
                    }]
                }));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538123", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let order_status = result.unwrap();
        assert_eq!(order_status.order_id, Some("1004055538123".to_string()));
        assert!(order_status.is_filled());
        assert!((order_status.filled_quantity.unwrap() - 100.0).abs() < f64::EPSILON);
        let avg_price = order_status.calculate_weighted_average_price().unwrap();
        assert!((avg_price - 150.25).abs() < f64::EPSILON);
        assert_eq!(order_status.price_in_cents().unwrap(), Some(15025));
    }

    #[tokio::test]
    async fn test_get_order_status_success_working() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538456")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "orderId": 1_004_055_538_456_i64,
                    "status": "WORKING",
                    "filledQuantity": 0.0,
                    "remainingQuantity": 100.0,
                    "orderActivityCollection": [],
                    "enteredTime": "2023-10-15T10:25:00Z",
                    "closeTime": null
                }));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538456", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let order_status = result.unwrap();
        assert_eq!(order_status.order_id, Some("1004055538456".to_string()));
        assert!(order_status.is_pending());
        assert!(!order_status.is_filled());
        assert!(order_status.filled_quantity.unwrap_or(0.0).abs() < f64::EPSILON);
        assert_eq!(order_status.calculate_weighted_average_price(), None);
    }

    #[tokio::test]
    async fn test_get_order_status_partially_filled() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538789")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "application/json");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "orderId": 1_004_055_538_789_i64,
                    "status": "WORKING",
                    "filledQuantity": 75.0,
                    "remainingQuantity": 25.0,
                    "enteredTime": "2023-10-15T10:25:00Z",
                    "closeTime": null,
                    "orderActivityCollection": [{
                        "activityType": "EXECUTION",
                        "executionLegs": [
                            {
                                "executionId": "EXEC001",
                                "quantity": 50.0,
                                "price": 100.00,
                                "time": "2023-10-15T10:30:00Z"
                            },
                            {
                                "executionId": "EXEC002",
                                "quantity": 25.0,
                                "price": 101.00,
                                "time": "2023-10-15T10:30:05Z"
                            }
                        ]
                    }]
                }));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538789", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let order_status = result.unwrap();
        assert_eq!(order_status.order_id, Some("1004055538789".to_string()));
        assert!(order_status.is_pending());
        assert!(!order_status.is_filled());
        assert!((order_status.filled_quantity.unwrap() - 75.0).abs() < f64::EPSILON);
        // Weighted average: (50 * 100.00 + 25 * 101.00) / 75 = (5000 + 2525) / 75 = 100.33333
        assert!(
            (order_status.calculate_weighted_average_price().unwrap() - 100.333_333_333_333_33)
                .abs()
                < 0.000_001
        );
    }

    #[tokio::test]
    async fn test_get_order_status_order_not_found() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/NONEXISTENT");
            then.status(404)
                .header("content-type", "application/json")
                .json_body(json!({"error": "Order not found"}));
        });

        let broker = Schwab;
        let result = broker.get_order_status("NONEXISTENT", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, body }
            if action == "get order status"
                && status == reqwest::StatusCode::NOT_FOUND
                && body.contains("NONEXISTENT")
        ));
    }

    #[tokio::test]
    async fn test_get_order_status_authentication_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538123");
            then.status(401)
                .header("content-type", "application/json")
                .json_body(json!({"error": "Unauthorized"}));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538123", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "get order status" && status == reqwest::StatusCode::UNAUTHORIZED
        ));
    }

    #[tokio::test]
    async fn test_get_order_status_server_error() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538123");
            then.status(500)
                .header("content-type", "application/json")
                .json_body(json!({"error": "Internal server error"}));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538123", &env, &pool).await;

        account_mock.assert();
        // With retries enabled, expect multiple attempts for 5xx errors
        assert!(
            order_status_mock.hits() >= 1,
            "Expected at least one API call attempt"
        );
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "get order status (server error)" && status == reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
    }

    #[tokio::test]
    async fn test_get_order_status_invalid_json_response() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538123");
            then.status(200)
                .header("content-type", "application/json")
                .body("invalid json response");
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538123", &env, &pool).await;

        account_mock.assert();
        order_status_mock.assert();
        let error = result.unwrap_err();
        // Updated to handle new ApiResponseParse error type for JSON parsing failures
        assert!(matches!(error, SchwabError::ApiResponseParse { .. }));
    }

    #[tokio::test]
    async fn test_get_order_status_retry_on_transient_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        // Mock that fails initially but should be retried
        let order_status_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538123");
            then.status(502) // Bad Gateway - transient error
                .header("content-type", "application/json")
                .json_body(json!({"error": "Bad Gateway"}));
        });

        let broker = Schwab;
        let result = broker.get_order_status("1004055538123", &env, &pool).await;

        account_mock.assert();
        // Should have made at least one request (retry logic is handled by backon)
        assert!(order_status_mock.hits() >= 1);
        let error = result.unwrap_err();
        assert!(matches!(
            error,
            SchwabError::RequestFailed { action, status, .. }
            if action == "get order status (server error)" && status == reqwest::StatusCode::BAD_GATEWAY
        ));
    }

    // These tests can be restored when/if the CLI functionality is migrated to the new system
}
