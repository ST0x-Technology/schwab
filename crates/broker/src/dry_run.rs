use async_trait::async_trait;
use sqlx::SqlitePool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::warn;

use crate::{Broker, BrokerError, MarketOrder, OrderPlacement, OrderStatus, OrderUpdate};

/// Mock broker for dry-run mode that logs operations without executing real trades
#[derive(Debug, Clone)]
pub struct DryRunBroker {
    order_counter: Arc<AtomicU64>,
}

impl DryRunBroker {
    pub fn new() -> Self {
        Self {
            order_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    fn generate_order_id(&self) -> String {
        let id = self.order_counter.fetch_add(1, Ordering::SeqCst);
        format!("DRY_RUN_{}", id)
    }
}

impl Default for DryRunBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Broker for DryRunBroker {
    type Error = BrokerError;
    type OrderId = String;

    async fn ensure_ready(&self, _pool: &SqlitePool) -> Result<(), Self::Error> {
        warn!("[DRY-RUN] Broker readiness check - always ready in dry-run mode");
        Ok(())
    }

    async fn place_market_order(
        &self,
        order: MarketOrder,
        _pool: &SqlitePool,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error> {
        let order_id = self.generate_order_id();

        warn!(
            "[DRY-RUN] Would execute order: {} {} shares of {} (order_id: {})",
            order.direction, order.shares, order.symbol, order_id
        );

        Ok(OrderPlacement {
            order_id,
            symbol: order.symbol,
            shares: order.shares,
            direction: order.direction,
            placed_at: chrono::Utc::now(),
        })
    }

    async fn get_order_status(
        &self,
        order_id: &Self::OrderId,
        _pool: &SqlitePool,
    ) -> Result<OrderStatus, Self::Error> {
        warn!("[DRY-RUN] Checking status for order: {}", order_id);
        warn!("[DRY-RUN] Returning mock FILLED status");

        // Always return filled status in dry-run mode
        Ok(OrderStatus::Filled)
    }

    async fn poll_pending_orders(
        &self,
        _pool: &SqlitePool,
    ) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error> {
        warn!("[DRY-RUN] Polling pending orders - no pending orders in dry-run mode");

        // Return empty list since dry-run orders are immediately "filled"
        Ok(Vec::new())
    }
}
