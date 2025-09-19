use async_trait::async_trait;
use sqlx::SqlitePool;
use std::fmt::{Debug, Display};

#[async_trait]
pub trait Broker: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;
    type OrderId: Display + Debug + Send + Sync + Clone;

    async fn ensure_ready(&self, pool: &SqlitePool) -> Result<(), Self::Error>;
    async fn place_market_order(
        &self,
        order: MarketOrder,
        pool: &SqlitePool,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error>;
    async fn get_order_status(
        &self,
        order_id: &Self::OrderId,
        pool: &SqlitePool,
    ) -> Result<OrderStatus, Self::Error>;
    async fn poll_pending_orders(
        &self,
        pool: &SqlitePool,
    ) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error>;
}
