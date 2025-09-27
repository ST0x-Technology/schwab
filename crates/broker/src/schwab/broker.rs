use async_trait::async_trait;
use sqlx::SqlitePool;
use tracing::info;

use crate::schwab::auth::SchwabAuthEnv;
use crate::schwab::market_hours::{MarketStatus, fetch_market_hours};
use crate::schwab::tokens::SchwabTokens;
use crate::{
    Broker, BrokerError, MarketOrder, OrderPlacement, OrderState, OrderUpdate, Shares, Symbol,
};

/// Configuration for SchwabBroker containing auth environment and database pool
pub type SchwabConfig = (SchwabAuthEnv, SqlitePool);

/// Schwab broker implementation
#[derive(Debug, Clone)]
pub struct SchwabBroker {
    auth: SchwabAuthEnv,
    pool: SqlitePool,
}

#[async_trait]
impl Broker for SchwabBroker {
    type Error = BrokerError;
    type OrderId = String;
    type Config = SchwabConfig;

    async fn try_from_config(config: Self::Config) -> Result<Self, Self::Error> {
        let (auth, pool) = config;

        // Validate and refresh tokens during initialization
        SchwabTokens::refresh_if_needed(&pool, &auth).await?;

        info!("Schwab broker initialized with valid tokens");

        Ok(Self { auth, pool })
    }

    async fn wait_until_market_open(&self) -> Result<Option<std::time::Duration>, Self::Error> {
        // Fetch current market hours directly
        let market_hours = fetch_market_hours(&self.auth, &self.pool, None).await?;

        // Check if market is currently open
        match market_hours.current_status() {
            MarketStatus::Open => Ok(None), // Market is open, no need to wait
            MarketStatus::Closed => {
                // Market is closed, calculate wait time until next open
                if let Some(start_time) = market_hours.start {
                    let next_open = start_time.with_timezone(&chrono::Utc);
                    let now = chrono::Utc::now();
                    if next_open > now {
                        let duration = (next_open - now)
                            .to_std()
                            .unwrap_or(std::time::Duration::from_secs(3600));
                        Ok(Some(duration))
                    } else {
                        // If next open is in past, return small wait time to retry
                        Ok(Some(std::time::Duration::from_secs(60)))
                    }
                } else {
                    // No next open time found, return default wait time
                    Ok(Some(std::time::Duration::from_secs(3600)))
                }
            }
        }
    }

    async fn place_market_order(
        &self,
        order: MarketOrder,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error> {
        info!(
            "Placing market order: {} {} shares of {}",
            order.direction, order.shares, order.symbol
        );

        // Convert Direction to Schwab Instruction
        let instruction = match order.direction {
            crate::Direction::Buy => crate::schwab::order::Instruction::Buy,
            crate::Direction::Sell => crate::schwab::order::Instruction::Sell,
        };

        // Create Schwab order
        let schwab_order = crate::schwab::order::Order::new(
            order.symbol.to_string(),
            instruction,
            order.shares.value().into(),
        );

        // Place the order using Schwab API
        let response = schwab_order
            .place(&self.auth, &self.pool)
            .await
            .map_err(|e| {
                BrokerError::OrderPlacement(format!("Schwab order placement failed: {}", e))
            })?;

        Ok(OrderPlacement {
            order_id: response.order_id,
            symbol: order.symbol,
            shares: order.shares,
            direction: order.direction,
            placed_at: chrono::Utc::now(),
        })
    }

    async fn get_order_status(&self, order_id: &Self::OrderId) -> Result<OrderState, Self::Error> {
        info!("Getting order status for: {}", order_id);

        // Call the existing Schwab API function
        let order_response =
            crate::schwab::order::Order::get_order_status(order_id, &self.auth, &self.pool)
                .await
                .map_err(|e| BrokerError::Network(format!("Failed to get order status: {}", e)))?;

        // Convert OrderStatusResponse to OrderState
        if order_response.is_filled() {
            let price_cents = order_response
                .price_in_cents()
                .map_err(|e| BrokerError::Network(format!("Failed to calculate price: {}", e)))?
                .unwrap_or(0);

            Ok(OrderState::Filled {
                executed_at: chrono::Utc::now(), // TODO: Parse actual timestamp from close_time
                order_id: order_id.clone(),
                price_cents,
            })
        } else if order_response.is_terminal_failure() {
            Ok(OrderState::Failed {
                failed_at: chrono::Utc::now(), // TODO: Parse actual timestamp from close_time
                error_reason: Some(format!("Order status: {:?}", order_response.status)),
            })
        } else {
            // Order is still pending/working
            Ok(OrderState::Submitted {
                order_id: order_id.clone(),
            })
        }
    }

    async fn poll_pending_orders(&self) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error> {
        info!("Polling pending orders");

        // Query database directly for submitted orders
        let rows = sqlx::query!(
            "SELECT * FROM offchain_trades WHERE status = 'SUBMITTED' ORDER BY id ASC"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut updates = Vec::new();

        for row in rows {
            let Some(order_id_value) = row.order_id else {
                continue; // Skip orders without order_id
            };

            // Get current status from Schwab API
            match self.get_order_status(&order_id_value).await {
                Ok(current_state) => {
                    // Only include orders that have changed status
                    if !matches!(current_state, OrderState::Submitted { .. }) {
                        let price_cents = match &current_state {
                            OrderState::Filled { price_cents, .. } => Some(*price_cents),
                            _ => None,
                        };

                        let symbol =
                            Symbol::new(row.symbol).map_err(|e| BrokerError::InvalidOrder {
                                reason: format!("Invalid symbol in database: {}", e),
                            })?;

                        let shares = Shares::new(row.shares as u64).map_err(|e| {
                            BrokerError::InvalidOrder {
                                reason: format!("Invalid shares in database: {}", e),
                            }
                        })?;

                        let direction =
                            row.direction
                                .parse()
                                .map_err(|e: crate::InvalidDirectionError| {
                                    BrokerError::InvalidOrder {
                                        reason: format!("Invalid direction in database: {}", e),
                                    }
                                })?;

                        updates.push(OrderUpdate {
                            order_id: order_id_value.clone(),
                            symbol,
                            shares,
                            direction,
                            status: current_state.status(),
                            updated_at: chrono::Utc::now(),
                            price_cents,
                        });
                    }
                }
                Err(e) => {
                    // Log error but continue with other orders
                    info!("Failed to get status for order {}: {}", order_id_value, e);
                    continue;
                }
            }
        }

        info!("Found {} order updates", updates.len());
        Ok(updates)
    }

    fn to_supported_broker(&self) -> crate::SupportedBroker {
        crate::SupportedBroker::Schwab
    }

    fn parse_order_id(&self, order_id_str: &str) -> Result<Self::OrderId, Self::Error> {
        // For SchwabBroker, OrderId is String, so just clone the input
        Ok(order_id_str.to_string())
    }

    async fn run_broker_maintenance(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<Result<(), Self::Error>>> {
        // Schwab broker needs token refresh maintenance
        let pool_clone = self.pool.clone();
        let auth_clone = self.auth.clone();

        let handle = tokio::spawn(async move {
            crate::schwab::tokens::SchwabTokens::start_automatic_token_refresh_loop(
                pool_clone,
                auth_clone,
                shutdown_rx,
            )
            .await
            .map_err(BrokerError::Schwab)
        });

        Some(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schwab::SchwabError;
    use crate::schwab::auth::SchwabAuthEnv;
    use crate::schwab::tokens::SchwabTokens;
    use chrono::{Duration, Utc};
    use httpmock::prelude::*;
    use serde_json::json;
    use sqlx::SqlitePool;

    fn create_test_auth_env() -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://test.com".to_string(),
            account_index: 0,
        }
    }

    fn create_test_auth_env_with_server(server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: server.base_url(),
            account_index: 0,
        }
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_try_from_config_with_no_tokens() {
        let pool = setup_test_db().await;
        let auth = create_test_auth_env();
        let config = (auth, pool);

        let result = SchwabBroker::try_from_config(config).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BrokerError::Schwab(_)));
    }

    #[tokio::test]
    async fn test_try_from_config_with_valid_tokens() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store valid tokens
        let valid_tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(10), // Fresh token
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1),
        };
        valid_tokens.store(&pool).await.unwrap();

        let config = (auth, pool);
        let result = SchwabBroker::try_from_config(config).await;

        assert!(result.is_ok());
        let broker = result.unwrap();
        assert_eq!(broker.auth.app_key, "test_key");
    }

    #[tokio::test]
    async fn test_try_from_config_with_expired_access_token_valid_refresh() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store tokens with expired access token but valid refresh token
        let tokens_needing_refresh = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(35), // Expired
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1), // Valid
        };
        tokens_needing_refresh.store(&pool).await.unwrap();

        // Mock the token refresh endpoint
        let refresh_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "access_token": "refreshed_access_token",
                    "refresh_token": "new_refresh_token",
                    "expires_in": 1800,
                    "token_type": "Bearer"
                }));
        });

        let config = (auth, pool.clone());
        let result = SchwabBroker::try_from_config(config).await;

        assert!(result.is_ok());
        refresh_mock.assert();

        // Verify tokens were updated
        let updated_tokens = SchwabTokens::load(&pool).await.unwrap();
        assert_eq!(updated_tokens.access_token, "refreshed_access_token");
        assert_eq!(updated_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_try_from_config_with_expired_refresh_token() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store tokens with both access and refresh tokens expired
        let expired_tokens = SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(35),
            refresh_token: "expired_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(8), // Expired
        };
        expired_tokens.store(&pool).await.unwrap();

        let config = (auth, pool);
        let result = SchwabBroker::try_from_config(config).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BrokerError::Schwab(SchwabError::RefreshTokenExpired)
        ));
    }

    #[tokio::test]
    async fn test_wait_until_market_open_with_market_open() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store valid tokens
        let valid_tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(10),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1),
        };
        valid_tokens.store(&pool).await.unwrap();

        // Mock market hours API to return open market
        // Use today's date with market hours that encompass current time
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let market_hours_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .header("authorization", "Bearer valid_access_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "equity": {
                        "EQ": {
                            "date": today,
                            "marketType": "EQUITY",
                            "exchange": "null",
                            "category": "null",
                            "product": "EQ",
                            "productName": "equity",
                            "isOpen": true,
                            "sessionHours": {
                                "preMarket": [
                                    {
                                        "start": format!("{}T04:00:00-05:00", today),
                                        "end": format!("{}T09:30:00-05:00", today)
                                    }
                                ],
                                "regularMarket": [
                                    {
                                        "start": format!("{}T00:00:00-05:00", today),
                                        "end": format!("{}T23:59:59-05:00", today)
                                    }
                                ]
                            }
                        }
                    }
                }));
        });

        let broker = SchwabBroker { auth, pool };
        let result = broker.wait_until_market_open().await;

        assert!(result.is_ok());
        let duration_opt = result.unwrap();
        assert_eq!(duration_opt, None); // Market is open, no wait needed
        market_hours_mock.assert();
    }

    #[tokio::test]
    async fn test_wait_until_market_open_with_market_closed() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store valid tokens
        let valid_tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(10),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1),
        };
        valid_tokens.store(&pool).await.unwrap();

        // Mock market hours API to return closed market
        let market_hours_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .header("authorization", "Bearer valid_access_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "equity": {
                        "EQ": {
                            "date": "2025-01-15",
                            "marketType": "EQUITY",
                            "exchange": "null",
                            "category": "null",
                            "product": "EQ",
                            "productName": "equity",
                            "isOpen": false,
                            "sessionHours": {
                                "preMarket": [],
                                "regularMarket": []
                            }
                        }
                    }
                }));
        });

        let broker = SchwabBroker { auth, pool };
        let result = broker.wait_until_market_open().await;

        assert!(result.is_ok());
        let duration_opt = result.unwrap();
        assert!(duration_opt.is_some()); // Market is closed, should return wait duration
        let duration = duration_opt.unwrap();
        assert!(duration.as_secs() > 0); // Should be positive duration
        market_hours_mock.assert();
    }

    #[tokio::test]
    async fn test_wait_until_market_open_with_api_error() {
        let pool = setup_test_db().await;
        let server = MockServer::start();
        let auth = create_test_auth_env_with_server(&server);

        // Store valid tokens
        let valid_tokens = SchwabTokens {
            access_token: "valid_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(10),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1),
        };
        valid_tokens.store(&pool).await.unwrap();

        // Mock market hours API to return error
        let market_hours_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .header("authorization", "Bearer valid_access_token");
            then.status(500)
                .header("content-type", "application/json")
                .json_body(json!({
                    "error": "Internal Server Error"
                }));
        });

        let broker = SchwabBroker { auth, pool };
        let result = broker.wait_until_market_open().await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BrokerError::Schwab(_)));
        market_hours_mock.assert();
    }

    #[tokio::test]
    async fn test_parse_order_id() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        let auth = create_test_auth_env();
        let broker = SchwabBroker { auth, pool };

        let test_id = "12345";
        let parsed = broker.parse_order_id(test_id).unwrap();
        assert_eq!(parsed, test_id);
    }

    #[tokio::test]
    async fn test_to_supported_broker() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        let auth = create_test_auth_env();
        let broker = SchwabBroker { auth, pool };

        assert_eq!(broker.to_supported_broker(), crate::SupportedBroker::Schwab);
    }
}
