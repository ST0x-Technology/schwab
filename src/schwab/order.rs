use backon::{ExponentialBuilder, Retryable};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::{error, info};

use super::{SchwabAuthEnv, SchwabError, SchwabTokens};
use crate::Env;
use crate::arb::ArbTrade;
use crate::trade::{SchwabInstruction, TradeStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub order_type: OrderType,
    pub session: Session,
    pub duration: OrderDuration,
    pub order_strategy_type: OrderStrategyType,
    pub order_leg_collection: Vec<OrderLeg>,
}

impl Order {
    pub fn new(symbol: String, instruction: Instruction, quantity: f64) -> Self {
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

    pub async fn place(&self, env: &SchwabAuthEnv, pool: &SqlitePool) -> Result<(), SchwabError> {
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

        let client = reqwest::Client::new();
        let response = (|| async {
            client
                .post(format!(
                    "{}/trader/v1/accounts/{}/orders",
                    env.base_url, account_hash
                ))
                .headers(headers.clone())
                .body(order_json.clone())
                .send()
                .await
        })
        .retry(ExponentialBuilder::default())
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

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderType {
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
pub enum Instruction {
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
pub enum Session {
    Normal,
    Am,
    Pm,
    Seamless,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderDuration {
    Day,
    GoodTillCancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStrategyType {
    Single,
    Oco,
    Trigger,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssetType {
    Equity,
    Option,
    Index,
    MutualFund,
    CashEquivalent,
    FixedIncome,
    Currency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrderLeg {
    pub instruction: Instruction,
    pub quantity: f64,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    pub symbol: String,
    pub asset_type: AssetType,
}

pub async fn execute_trade(env: &Env, pool: &SqlitePool, trade: ArbTrade, max_retries: usize) {
    let schwab_instruction = match trade.schwab_instruction {
        SchwabInstruction::Buy => Instruction::Buy,
        SchwabInstruction::Sell => Instruction::Sell,
    };

    let order = Order::new(
        trade.schwab_ticker.clone(),
        schwab_instruction,
        trade.schwab_quantity,
    );

    let result = (|| async { order.place(&env.schwab_auth, pool).await })
        .retry(ExponentialBuilder::new().with_max_times(max_retries))
        .await;

    match result {
        Ok(()) => handle_order_success(&trade, pool).await,
        Err(e) => handle_order_failure(&trade, pool, e).await,
    }
}

async fn handle_order_success(trade: &ArbTrade, pool: &SqlitePool) {
    info!(
        "Successfully placed Schwab order for trade: tx_hash={tx_hash:?}, log_index={log_index}",
        tx_hash = trade.tx_hash,
        log_index = trade.log_index
    );

    if let Err(e) =
        ArbTrade::update_status(pool, trade.tx_hash, trade.log_index, TradeStatus::Completed).await
    {
        error!(
            "Failed to update trade status to COMPLETED: tx_hash={tx_hash:?}, log_index={log_index}, error={e:?}",
            tx_hash = trade.tx_hash,
            log_index = trade.log_index
        );
    }
}

async fn handle_order_failure(trade: &ArbTrade, pool: &SqlitePool, error: SchwabError) {
    error!(
        "Failed to place Schwab order after retries for trade: tx_hash={tx_hash:?}, log_index={log_index}, error={error:?}",
        tx_hash = trade.tx_hash,
        log_index = trade.log_index
    );

    if let Err(update_err) =
        ArbTrade::update_status(pool, trade.tx_hash, trade.log_index, TradeStatus::Failed).await
    {
        error!(
            "Failed to update trade status to FAILED: tx_hash={tx_hash:?}, log_index={log_index}, error={update_err:?}",
            tx_hash = trade.tx_hash,
            log_index = trade.log_index
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn test_new_buy() {
        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100.0);

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.session, Session::Normal);
        assert_eq!(order.duration, OrderDuration::Day);
        assert_eq!(order.order_strategy_type, OrderStrategyType::Single);
        assert_eq!(order.order_leg_collection.len(), 1);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Buy);
        assert!((leg.quantity - 100.0).abs() < f64::EPSILON);
        assert_eq!(leg.instrument.symbol, "AAPL");
        assert_eq!(leg.instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_new_sell() {
        let order = Order::new("TSLA".to_string(), Instruction::Sell, 50.5);

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.session, Session::Normal);
        assert_eq!(order.duration, OrderDuration::Day);
        assert_eq!(order.order_strategy_type, OrderStrategyType::Single);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Sell);
        assert!((leg.quantity - 50.5).abs() < f64::EPSILON);
        assert_eq!(leg.instrument.symbol, "TSLA");
        assert_eq!(leg.instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_new_sell_short() {
        let order = Order::new("GME".to_string(), Instruction::SellShort, 25.75);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::SellShort);
        assert!((leg.quantity - 25.75).abs() < f64::EPSILON);
        assert_eq!(leg.instrument.symbol, "GME");
    }

    #[test]
    fn test_new_buy_to_cover() {
        let order = Order::new("AMC".to_string(), Instruction::BuyToCover, 15.25);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::BuyToCover);
        assert!((leg.quantity - 15.25).abs() < f64::EPSILON);
    }

    #[test]
    fn test_fractional_shares() {
        let order = Order::new("SPY".to_string(), Instruction::Buy, 0.5);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Buy);
        assert!((leg.quantity - 0.5).abs() < f64::EPSILON);
        assert_eq!(leg.instrument.symbol, "SPY");

        // Test serialization preserves fractional shares
        let json = serde_json::to_value(&order).unwrap();
        assert_eq!(json["orderLegCollection"][0]["quantity"], 0.5);
    }

    #[test]
    fn test_order_serialization() {
        let order = Order::new("MSFT".to_string(), Instruction::Buy, 25.333);

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
        assert!(
            (order.order_leg_collection[0].quantity
                - deserialized.order_leg_collection[0].quantity)
                .abs()
                < f64::EPSILON
        );
        assert_eq!(
            order.order_leg_collection[0].instrument,
            deserialized.order_leg_collection[0].instrument
        );
    }

    #[test]
    fn test_order_camel_case_serialization() {
        let order = Order::new("GOOGL".to_string(), Instruction::Buy, 10.0);

        let json = serde_json::to_string_pretty(&order).unwrap();

        assert!(json.contains("\"orderType\""));
        assert!(json.contains("\"orderLegCollection\""));
        assert!(json.contains("\"orderStrategyType\""));
        assert!(json.contains("\"assetType\""));
    }

    #[test]
    fn test_serialization_matches_schwab_format() {
        let order = Order::new("XYZ".to_string(), Instruction::Buy, 15.0);

        let json = serde_json::to_value(&order).unwrap();

        assert_eq!(json["orderType"], "MARKET");
        assert_eq!(json["session"], "NORMAL");
        assert_eq!(json["duration"], "DAY");
        assert_eq!(json["orderStrategyType"], "SINGLE");
        assert_eq!(json["orderLegCollection"][0]["instruction"], "BUY");
        assert_eq!(json["orderLegCollection"][0]["quantity"], 15.0);
        assert_eq!(json["orderLegCollection"][0]["instrument"]["symbol"], "XYZ");
        assert_eq!(
            json["orderLegCollection"][0]["instrument"]["assetType"],
            "EQUITY"
        );
    }

    #[test]
    fn test_enum_serialization_values() {
        assert_eq!(serde_json::to_value(OrderType::Market).unwrap(), "MARKET");
        assert_eq!(serde_json::to_value(OrderType::Limit).unwrap(), "LIMIT");
        assert_eq!(serde_json::to_value(OrderType::Stop).unwrap(), "STOP");
        assert_eq!(
            serde_json::to_value(OrderType::StopLimit).unwrap(),
            "STOP_LIMIT"
        );

        assert_eq!(serde_json::to_value(Instruction::Buy).unwrap(), "BUY");
        assert_eq!(serde_json::to_value(Instruction::Sell).unwrap(), "SELL");
        assert_eq!(
            serde_json::to_value(Instruction::SellShort).unwrap(),
            "SELL_SHORT"
        );
        assert_eq!(
            serde_json::to_value(Instruction::BuyToCover).unwrap(),
            "BUY_TO_COVER"
        );

        assert_eq!(serde_json::to_value(Session::Normal).unwrap(), "NORMAL");
        assert_eq!(serde_json::to_value(Session::Am).unwrap(), "AM");
        assert_eq!(serde_json::to_value(Session::Pm).unwrap(), "PM");

        assert_eq!(serde_json::to_value(OrderDuration::Day).unwrap(), "DAY");
        assert_eq!(
            serde_json::to_value(OrderDuration::GoodTillCancel).unwrap(),
            "GOOD_TILL_CANCEL"
        );

        assert_eq!(serde_json::to_value(AssetType::Equity).unwrap(), "EQUITY");
        assert_eq!(serde_json::to_value(AssetType::Option).unwrap(), "OPTION");
    }

    #[test]
    fn test_enum_deserialization() {
        let order_type: OrderType = serde_json::from_str("\"MARKET\"").unwrap();
        assert_eq!(order_type, OrderType::Market);

        let instruction: Instruction = serde_json::from_str("\"SELL_SHORT\"").unwrap();
        assert_eq!(instruction, Instruction::SellShort);
        let json = r#"{
            "orderType": "MARKET",
            "session": "NORMAL", 
            "duration": "DAY",
            "orderStrategyType": "SINGLE",
            "orderLegCollection": [{
                "instruction": "BUY",
                "quantity": 100.0,
                "instrument": {
                    "symbol": "AAPL",
                    "assetType": "EQUITY"
                }
            }]
        }"#;

        let order: Order = serde_json::from_str(json).unwrap();
        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.order_leg_collection[0].instruction, Instruction::Buy);
        assert_eq!(
            order.order_leg_collection[0].instrument.asset_type,
            AssetType::Equity
        );
    }

    #[test]
    fn test_instrument_creation() {
        let instrument = Instrument {
            symbol: "SPY".to_string(),
            asset_type: AssetType::Equity,
        };

        assert_eq!(instrument.symbol, "SPY");
        assert_eq!(instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_order_leg_creation() {
        let instrument = Instrument {
            symbol: "VTI".to_string(),
            asset_type: AssetType::Equity,
        };

        let order_leg = OrderLeg {
            instruction: Instruction::Sell,
            quantity: 75.5,
            instrument,
        };

        assert_eq!(order_leg.instruction, Instruction::Sell);
        assert!((order_leg.quantity - 75.5).abs() < f64::EPSILON);
        assert_eq!(order_leg.instrument.symbol, "VTI");
        assert_eq!(order_leg.instrument.asset_type, AssetType::Equity);
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
            then.status(201);
        });

        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100.0);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        result.unwrap();
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

        let order = Order::new("INVALID".to_string(), Instruction::Buy, 100.0);
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

    async fn setup_test_db() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
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
    async fn test_execute_trade_success() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_for_execute_trade(&server);
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
            then.status(201);
        });

        let trade = create_test_trade();
        trade.try_save_to_db(&pool).await.unwrap();

        execute_trade(&env, &pool, trade.clone(), 0).await;

        account_mock.assert();
        order_mock.assert();

        let updated_trade = sqlx::query!(
            "SELECT status FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(updated_trade.status.unwrap(), "COMPLETED");
    }

    #[tokio::test]
    async fn test_execute_trade_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_for_execute_trade(&server);
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
                .json_body(json!({"error": "Order validation failed"}));
        });

        let trade = create_test_trade();
        trade.try_save_to_db(&pool).await.unwrap();

        execute_trade(&env, &pool, trade.clone(), 0).await;

        account_mock.assert();
        order_mock.assert();

        let updated_trade = sqlx::query!(
            "SELECT status FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(updated_trade.status.unwrap(), "FAILED");
    }

    #[tokio::test]
    async fn test_execute_trade_retry_until_failure() {
        let server = httpmock::MockServer::start();
        let env = create_test_env_for_execute_trade(&server);
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

        // Mock that fails twice then succeeds
        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(500)
                .json_body(json!({"error": "Internal server error"}));
        });

        let trade = create_test_trade();
        trade.try_save_to_db(&pool).await.unwrap();

        execute_trade(&env, &pool, trade.clone(), 2).await;

        // Should fail 3 times total (1 initial + 2 retries)
        account_mock.assert_hits(3);
        order_mock.assert_hits(3);

        let updated_trade = sqlx::query!(
            "SELECT status FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(updated_trade.status.unwrap(), "FAILED");
    }

    #[tokio::test]
    async fn test_handle_order_success() {
        let pool = setup_test_db().await;
        let trade = create_test_trade();
        trade.try_save_to_db(&pool).await.unwrap();

        handle_order_success(&trade, &pool).await;

        let updated_trade = sqlx::query!(
            "SELECT status FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(updated_trade.status.unwrap(), "COMPLETED");
    }

    #[tokio::test]
    async fn test_handle_order_failure() {
        let pool = setup_test_db().await;
        let trade = create_test_trade();
        trade.try_save_to_db(&pool).await.unwrap();

        let error = SchwabError::RequestFailed {
            action: "place order".to_string(),
            status: reqwest::StatusCode::BAD_REQUEST,
            body: "Invalid order".to_string(),
        };

        handle_order_failure(&trade, &pool, error).await;

        let updated_trade = sqlx::query!(
            "SELECT status FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(updated_trade.status.unwrap(), "FAILED");
    }

    fn create_test_env_for_execute_trade(mock_server: &httpmock::MockServer) -> crate::Env {
        use crate::{Env, LogLevel, trade::EvmEnv};
        use alloy::primitives::{address, fixed_bytes};
        use url::Url;

        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            schwab_auth: SchwabAuthEnv {
                app_key: "test_app_key".to_string(),
                app_secret: "test_app_secret".to_string(),
                redirect_uri: "https://127.0.0.1".to_string(),
                base_url: mock_server.base_url(),
                account_index: 0,
            },
            evm_env: EvmEnv {
                ws_rpc_url: Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1234567890123456789012345678901234567890"),
                order_hash: fixed_bytes!(
                    "0x0000000000000000000000000000000000000000000000000000000000000000"
                ),
            },
        }
    }

    fn create_test_trade() -> crate::arb::ArbTrade {
        use crate::{
            arb::ArbTrade,
            trade::{SchwabInstruction, TradeStatus},
        };
        use alloy::primitives::fixed_bytes;

        ArbTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd"
            ),
            log_index: 123,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5.0,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            created_at: None,
            completed_at: None,
        }
    }
}
