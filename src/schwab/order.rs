use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{SchwabAuthEnv, SchwabError, SchwabTokens};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub order_type: OrderType,
    pub session: Session,
    pub duration: Duration,
    pub order_strategy_type: OrderStrategyType,
    pub order_leg_collection: Vec<OrderLeg>,
}

impl Order {
    pub fn new(symbol: String, instruction: Instruction, quantity: u32) -> Self {
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
            duration: Duration::Day,
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
            (header::CONTENT_TYPE, HeaderValue::from_str("application/json")?),
        ]
        .into_iter()
        .collect::<HeaderMap>();

        let order_json = serde_json::to_string(self)?;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/trader/v1/accounts/{}/orders", env.base_url, account_hash))
            .headers(headers)
            .body(order_json)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(SchwabError::OrderPlacementFailed {
                status: response.status(),
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
pub enum Duration {
    Day,
    Gtc,
    Fok,
    Ioc,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrderLeg {
    pub instruction: Instruction,
    pub quantity: u32,
    pub instrument: Instrument,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    pub symbol: String,
    pub asset_type: AssetType,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_new_buy() {
        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);

        assert_eq!(order.order_type, OrderType::Market);
        assert_eq!(order.session, Session::Normal);
        assert_eq!(order.duration, Duration::Day);
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
        assert_eq!(order.duration, Duration::Day);
        assert_eq!(order.order_strategy_type, OrderStrategyType::Single);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::Sell);
        assert_eq!(leg.quantity, 50);
        assert_eq!(leg.instrument.symbol, "TSLA");
        assert_eq!(leg.instrument.asset_type, AssetType::Equity);
    }

    #[test]
    fn test_new_sell_short() {
        let order = Order::new("GME".to_string(), Instruction::SellShort, 25);

        let leg = &order.order_leg_collection[0];
        assert_eq!(leg.instruction, Instruction::SellShort);
        assert_eq!(leg.quantity, 25);
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
    fn test_order_serialization() {
        let order = Order::new("MSFT".to_string(), Instruction::Buy, 25);

        let json = serde_json::to_string(&order).unwrap();
        let deserialized: Order = serde_json::from_str(&json).unwrap();

        assert_eq!(order, deserialized);
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

    #[test]
    fn test_enum_serialization_values() {
        // Test OrderType enum serialization
        assert_eq!(serde_json::to_value(OrderType::Market).unwrap(), "MARKET");
        assert_eq!(serde_json::to_value(OrderType::Limit).unwrap(), "LIMIT");
        assert_eq!(serde_json::to_value(OrderType::Stop).unwrap(), "STOP");
        assert_eq!(
            serde_json::to_value(OrderType::StopLimit).unwrap(),
            "STOP_LIMIT"
        );

        // Test Instruction enum serialization
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

        // Test Session enum serialization
        assert_eq!(serde_json::to_value(Session::Normal).unwrap(), "NORMAL");
        assert_eq!(serde_json::to_value(Session::Am).unwrap(), "AM");
        assert_eq!(serde_json::to_value(Session::Pm).unwrap(), "PM");

        // Test Duration enum serialization
        assert_eq!(serde_json::to_value(Duration::Day).unwrap(), "DAY");
        assert_eq!(serde_json::to_value(Duration::Gtc).unwrap(), "GTC");

        // Test AssetType enum serialization
        assert_eq!(serde_json::to_value(AssetType::Equity).unwrap(), "EQUITY");
        assert_eq!(serde_json::to_value(AssetType::Option).unwrap(), "OPTION");
    }

    #[test]
    fn test_enum_deserialization() {
        // Test OrderType deserialization
        let order_type: OrderType = serde_json::from_str("\"MARKET\"").unwrap();
        assert_eq!(order_type, OrderType::Market);

        // Test Instruction deserialization
        let instruction: Instruction = serde_json::from_str("\"SELL_SHORT\"").unwrap();
        assert_eq!(instruction, Instruction::SellShort);

        // Test full order deserialization
        let json = r#"{
            "orderType": "MARKET",
            "session": "NORMAL", 
            "duration": "DAY",
            "orderStrategyType": "SINGLE",
            "orderLegCollection": [{
                "instruction": "BUY",
                "quantity": 100,
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
            quantity: 75,
            instrument,
        };

        assert_eq!(order_leg.instruction, Instruction::Sell);
        assert_eq!(order_leg.quantity, 75);
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
                .json_body(serde_json::json!([{
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

        let order = Order::new("AAPL".to_string(), Instruction::Buy, 100);
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
                .json_body(serde_json::json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(400)
                .json_body(serde_json::json!({"error": "Invalid order"}));
        });

        let order = Order::new("INVALID".to_string(), Instruction::Buy, 100);
        let result = order.place(&env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(error, super::SchwabError::OrderPlacementFailed { status } if status.as_u16() == 400));
    }

    fn create_test_env_with_mock_server(mock_server: &httpmock::MockServer) -> super::SchwabAuthEnv {
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
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(pool).await.unwrap();
    }
}
