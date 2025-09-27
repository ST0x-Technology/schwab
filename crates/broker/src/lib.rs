use async_trait::async_trait;
use sqlx::SqlitePool;
use std::fmt::{Debug, Display};

pub mod error;
pub mod order_state;
pub mod schwab;
pub mod test;

pub use error::{OnChainError, PersistenceError};
pub use order_state::OrderState;
pub use schwab::broker::SchwabBroker;
pub use test::TestBroker;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidDirectionError(String);

impl std::fmt::Display for InvalidDirectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid direction: {}", self.0)
    }
}

impl std::error::Error for InvalidDirectionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedBroker {
    Schwab,
    DryRun,
}

impl std::fmt::Display for SupportedBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupportedBroker::Schwab => write!(f, "schwab"),
            SupportedBroker::DryRun => write!(f, "dry_run"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Buy,
    Sell,
}

impl Direction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

impl Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for Direction {
    type Err = InvalidDirectionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            _ => Err(InvalidDirectionError(s.to_string())),
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
    pub price_cents: Option<u64>,
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
    type Config: Send + Sync + Clone + 'static;

    fn new(config: Self::Config) -> Self;

    async fn ensure_ready(&self) -> Result<(), Self::Error>;

    async fn place_market_order(
        &self,
        order: MarketOrder,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error>;

    async fn get_order_status(&self, order_id: &Self::OrderId) -> Result<OrderState, Self::Error>;

    async fn poll_pending_orders(&self) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error>;

    fn to_supported_broker(&self) -> SupportedBroker;
}
