use async_trait::async_trait;
use sqlx::SqlitePool;

use super::auth::SchwabAuthEnv;
use crate::{Broker, BrokerError, MarketOrder, OrderPlacement, OrderStatus, OrderUpdate};

/// Schwab broker implementation
#[derive(Debug, Clone)]
pub struct SchwabBroker {
    pub auth: SchwabAuthEnv,
}

impl SchwabBroker {
    pub fn new(auth: SchwabAuthEnv) -> Self {
        Self { auth }
    }
}

#[async_trait]
impl Broker for SchwabBroker {
    type Error = BrokerError;
    type OrderId = String;

    async fn ensure_ready(&self, _pool: &SqlitePool) -> Result<(), Self::Error> {
        // TODO: Implement readiness check (e.g., validate tokens)
        Ok(())
    }

    async fn place_market_order(
        &self,
        _order: MarketOrder,
        _pool: &SqlitePool,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error> {
        // TODO: Implement order placement logic
        todo!("Implement place_market_order")
    }

    async fn get_order_status(
        &self,
        _order_id: &Self::OrderId,
        _pool: &SqlitePool,
    ) -> Result<OrderStatus, Self::Error> {
        // TODO: Implement order status retrieval
        todo!("Implement get_order_status")
    }

    async fn poll_pending_orders(
        &self,
        _pool: &SqlitePool,
    ) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error> {
        // TODO: Implement pending orders polling
        todo!("Implement poll_pending_orders")
    }
}
