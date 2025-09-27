use async_trait::async_trait;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::info;

use crate::schwab::auth::SchwabAuthEnv;
use crate::schwab::market_hours::{MarketHours, fetch_market_hours};
use crate::schwab::market_hours_cache::MarketHoursCache;
use crate::schwab::tokens::SchwabTokens;
use crate::{Broker, BrokerError, MarketOrder, OrderPlacement, OrderState, OrderUpdate};

/// Configuration for SchwabBroker containing auth environment and database pool
pub type SchwabConfig = (SchwabAuthEnv, SqlitePool);

/// Schwab broker implementation
#[derive(Debug, Clone)]
pub struct SchwabBroker {
    auth: SchwabAuthEnv,
    pool: SqlitePool,
}

impl SchwabBroker {
    /// Validates token access using stored pool and auth
    async fn validate_token_access(&self) -> Result<(), String> {
        // Use actual token validation logic
        match SchwabTokens::get_valid_access_token(&self.pool, &self.auth).await {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Token validation failed: {}", e)),
        }
    }
}

#[async_trait]
impl Broker for SchwabBroker {
    type Error = BrokerError;
    type OrderId = String;
    type Config = SchwabConfig;

    async fn try_from_config(config: Self::Config) -> Result<Self, Self::Error> {
        let (auth, pool) = config;

        // Validate and refresh tokens during initialization
        SchwabTokens::refresh_if_needed(&pool, &auth)
            .await
            .map_err(|e| BrokerError::Authentication(format!("Token validation failed: {}", e)))?;

        info!("Schwab broker initialized with valid tokens");

        Ok(Self { auth, pool })
    }

    async fn wait_until_market_open(&self) -> Result<Option<std::time::Duration>, Self::Error> {
        // Create market hours cache
        let market_hours_cache = Arc::new(MarketHoursCache::new());

        // Get market hours for today
        let market_hours = fetch_market_hours(&self.auth, &self.pool, None).await?;

        let now = chrono::Utc::now();

        // Check if market is currently open
        if market_hours.is_market_open_at(now) {
            Ok(None) // Market is open, no need to wait
        } else {
            // Market is closed, find next open time
            if let Some(next_open) = market_hours.next_market_open(now) {
                let wait_duration = (next_open - now)
                    .to_std()
                    .map_err(|e| BrokerError::Network(format!("Invalid duration: {}", e)))?;
                Ok(Some(wait_duration))
            } else {
                // No market open time found (e.g., weekend)
                // Return a default wait time (check again in 1 hour)
                Ok(Some(std::time::Duration::from_secs(3600)))
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
            order.shares.0.into(),
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

        // For now, return empty list
        // This will be replaced with actual Schwab API polling
        Ok(Vec::new())
    }

    fn to_supported_broker(&self) -> crate::SupportedBroker {
        crate::SupportedBroker::Schwab
    }

    fn parse_order_id(&self, order_id_str: &str) -> Result<Self::OrderId, Self::Error> {
        // For SchwabBroker, OrderId is String, so just clone the input
        Ok(order_id_str.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schwab::auth::SchwabAuthEnv;
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

    #[tokio::test]
    async fn test_try_from_config_with_invalid_tokens() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        let auth = create_test_auth_env();
        let config = (auth, pool);

        let result = SchwabBroker::try_from_config(config).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BrokerError::Authentication(_)
        ));
    }

    #[tokio::test]
    async fn test_wait_until_market_open_returns_duration_type() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        let auth = create_test_auth_env();
        let broker = SchwabBroker { auth, pool };

        let result = broker.wait_until_market_open().await;

        // The result should be a Result containing Option<Duration>
        assert!(result.is_ok() || matches!(result.unwrap_err(), BrokerError::Network(_)));
        if let Ok(duration_opt) = result {
            // If it returns Some(duration), duration should be positive
            if let Some(duration) = duration_opt {
                assert!(duration.as_secs() > 0);
            }
        }
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
