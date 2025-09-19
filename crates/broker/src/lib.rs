use async_trait::async_trait;
use sqlx::SqlitePool;
use std::fmt::{Debug, Display};

pub mod order_state;

pub use order_state::OrderState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Symbol(pub String);

impl Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shares(pub u32);

impl Display for Shares {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Buy,
    Sell,
}

impl Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Buy => write!(f, "BUY"),
            Direction::Sell => write!(f, "SELL"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MarketOrder {
    pub symbol: Symbol,
    pub shares: Shares,
    pub direction: Direction,
}

// Flat enum for database storage (matches CHECK constraint pattern)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Pending,
    Submitted,
    Filled,
    Failed,
}

impl OrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Submitted => "SUBMITTED",
            Self::Filled => "FILLED",
            Self::Failed => "FAILED",
        }
    }
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug)]
pub struct OrderPlacement<OrderId> {
    pub order_id: OrderId,
    pub symbol: Symbol,
    pub shares: Shares,
    pub direction: Direction,
    pub placed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct OrderUpdate<OrderId> {
    pub order_id: OrderId,
    pub symbol: Symbol,
    pub shares: Shares,
    pub direction: Direction,
    pub status: OrderStatus,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Authentication failed: {0}")]
    Authentication(String),

    #[error("Order placement failed: {0}")]
    OrderPlacement(String),

    #[error("Order not found: {order_id}")]
    OrderNotFound { order_id: String },

    #[error("Network error: {0}")]
    Network(String),

    #[error("Rate limited: retry after {retry_after_seconds} seconds")]
    RateLimit { retry_after_seconds: u64 },

    #[error("Broker unavailable: {message}")]
    Unavailable { message: String },

    #[error("Invalid order: {reason}")]
    InvalidOrder { reason: String },
}

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

#[derive(Debug, Default, Clone)]
pub struct MockBroker {
    pub should_fail: bool,
    pub failure_message: String,
}

impl MockBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_failure(message: impl Into<String>) -> Self {
        Self {
            should_fail: true,
            failure_message: message.into(),
        }
    }
}

#[async_trait]
impl Broker for MockBroker {
    type Error = BrokerError;
    type OrderId = String;

    async fn ensure_ready(&self, _pool: &SqlitePool) -> Result<(), Self::Error> {
        if self.should_fail {
            Err(BrokerError::Unavailable {
                message: self.failure_message.clone(),
            })
        } else {
            Ok(())
        }
    }

    async fn place_market_order(
        &self,
        order: MarketOrder,
        _pool: &SqlitePool,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error> {
        if self.should_fail {
            return Err(BrokerError::OrderPlacement(self.failure_message.clone()));
        }

        let order_id = format!("MOCK_{}", chrono::Utc::now().timestamp_millis());
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
        if self.should_fail {
            return Err(BrokerError::OrderNotFound {
                order_id: order_id.clone(),
            });
        }

        Ok(OrderStatus::Filled)
    }

    async fn poll_pending_orders(
        &self,
        _pool: &SqlitePool,
    ) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error> {
        if self.should_fail {
            return Err(BrokerError::Network(self.failure_message.clone()));
        }

        Ok(Vec::new())
    }
}
