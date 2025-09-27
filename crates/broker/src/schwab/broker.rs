use async_trait::async_trait;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::schwab::auth::SchwabAuthEnv;
use crate::schwab::tokens::SchwabTokens;
use crate::{
    Broker, BrokerError, MarketOrder, OrderPlacement, OrderState, OrderStatus, OrderUpdate,
};

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

        // For now, create a mock implementation that simulates order placement
        // This will be replaced with actual Schwab API integration
        let order_id = format!("SCHWAB_{}", chrono::Utc::now().timestamp_millis());

        // Simulate successful order placement
        Ok(OrderPlacement {
            order_id,
            symbol: order.symbol,
            shares: order.shares,
            direction: order.direction,
            placed_at: chrono::Utc::now(),
        })
    }

    async fn get_order_status(&self, order_id: &Self::OrderId) -> Result<OrderState, Self::Error> {
        info!("Getting order status for: {}", order_id);

        // For now, return filled status with mock data
        // This will be replaced with actual Schwab API status checking
        Ok(OrderState::Filled {
            executed_at: chrono::Utc::now(),
            order_id: order_id.clone(),
            price_cents: 10000, // $100.00 mock price
        })
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
