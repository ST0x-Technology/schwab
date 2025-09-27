use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::warn;

use crate::{
    Broker, BrokerError, MarketOrder, OrderPlacement, OrderState, OrderUpdate, SupportedBroker,
};

/// Unified test broker for dry-run mode and testing that logs operations without executing real trades
#[derive(Debug, Clone)]
pub struct TestBroker {
    order_counter: Arc<AtomicU64>,
    should_fail: bool,
    failure_message: String,
}

impl TestBroker {
    pub fn new() -> Self {
        Self {
            order_counter: Arc::new(AtomicU64::new(1)),
            should_fail: false,
            failure_message: String::new(),
        }
    }

    pub fn with_failure(message: impl Into<String>) -> Self {
        Self {
            order_counter: Arc::new(AtomicU64::new(1)),
            should_fail: true,
            failure_message: message.into(),
        }
    }

    fn generate_order_id(&self) -> String {
        let id = self.order_counter.fetch_add(1, Ordering::SeqCst);
        format!("TEST_{}", id)
    }
}

impl Default for TestBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Broker for TestBroker {
    type Error = BrokerError;
    type OrderId = String;
    type Config = ();

    fn new(_config: Self::Config) -> Self {
        Self::new()
    }

    async fn ensure_ready(&self) -> Result<(), Self::Error> {
        if self.should_fail {
            return Err(BrokerError::Unavailable {
                message: self.failure_message.clone(),
            });
        }

        warn!("[TEST] Broker readiness check - always ready in test mode");
        Ok(())
    }

    async fn place_market_order(
        &self,
        order: MarketOrder,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error> {
        if self.should_fail {
            return Err(BrokerError::OrderPlacement(self.failure_message.clone()));
        }

        let order_id = self.generate_order_id();

        warn!(
            "[TEST] Would execute order: {} {} shares of {} (order_id: {})",
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

    async fn get_order_status(&self, order_id: &Self::OrderId) -> Result<OrderState, Self::Error> {
        if self.should_fail {
            return Err(BrokerError::OrderNotFound {
                order_id: order_id.clone(),
            });
        }

        warn!("[TEST] Checking status for order: {}", order_id);
        warn!("[TEST] Returning mock FILLED status with test price");

        // Always return filled status in test mode with mock price
        Ok(OrderState::Filled {
            executed_at: chrono::Utc::now(),
            order_id: order_id.clone(),
            price_cents: 10000, // $100.00 mock price
        })
    }

    async fn poll_pending_orders(&self) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error> {
        if self.should_fail {
            return Err(BrokerError::Network(self.failure_message.clone()));
        }

        warn!("[TEST] Polling pending orders - no pending orders in test mode");

        // Return empty list since test orders are immediately "filled"
        Ok(Vec::new())
    }

    fn to_supported_broker(&self) -> SupportedBroker {
        SupportedBroker::DryRun
    }
}
