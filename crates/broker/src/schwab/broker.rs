use async_trait::async_trait;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::schwab::auth::SchwabAuthEnv;
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

    fn new(config: Self::Config) -> Self {
        let (auth, pool) = config;
        Self { auth, pool }
    }

    async fn ensure_ready(&self) -> Result<(), Self::Error> {
        // Check if we can get a valid access token (validates token state)
        match self.validate_token_access().await {
            Ok(_) => {
                info!("Schwab broker is ready - tokens are valid");
                Ok(())
            }
            Err(e) => {
                error!("Schwab broker not ready: {}", e);
                Err(BrokerError::Authentication(format!(
                    "Token validation failed: {}",
                    e
                )))
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
}
