use std::fmt::Debug;

pub mod state;
pub mod status;

pub use state::OrderState;
pub use status::OrderStatus;

/// Trait for types that can provide order status information
pub trait HasOrderStatus {
    fn status_str(&self) -> &'static str;
}

#[derive(Debug)]
pub struct OrderPlacement<OrderId> {
    pub order_id: OrderId,
    pub symbol: crate::Symbol,
    pub shares: crate::Shares,
    pub direction: crate::Direction,
    pub placed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct OrderUpdate<OrderId> {
    pub order_id: OrderId,
    pub symbol: crate::Symbol,
    pub shares: crate::Shares,
    pub direction: crate::Direction,
    pub status: OrderStatus,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub price_cents: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MarketOrder {
    pub symbol: crate::Symbol,
    pub shares: crate::Shares,
    pub direction: crate::Direction,
}
