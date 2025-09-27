use async_trait::async_trait;
use std::fmt::{Debug, Display};

pub mod error;
pub mod order;
pub mod schwab;
pub mod test;

#[cfg(test)]
pub mod test_utils;

pub use error::PersistenceError;
pub use order::{MarketOrder, OrderPlacement, OrderState, OrderStatus, OrderUpdate};
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

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Schwab API error: {0}")]
    Schwab(#[from] schwab::SchwabError),

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

    /// Create and validate broker instance from config
    /// All initialization and validation happens here
    async fn try_from_config(config: Self::Config) -> Result<Self, Self::Error>
    where
        Self: Sized;

    /// Wait until market opens if needed
    /// Returns None if market is already open
    /// Returns Some(duration) if need to wait for market to open
    async fn wait_until_market_open(&self) -> Result<Option<std::time::Duration>, Self::Error>;

    async fn place_market_order(
        &self,
        order: MarketOrder,
    ) -> Result<OrderPlacement<Self::OrderId>, Self::Error>;

    async fn get_order_status(&self, order_id: &Self::OrderId) -> Result<OrderState, Self::Error>;

    async fn poll_pending_orders(&self) -> Result<Vec<OrderUpdate<Self::OrderId>>, Self::Error>;

    fn to_supported_broker(&self) -> SupportedBroker;

    /// Convert a string representation to the broker's OrderId type
    /// This is needed for converting database-stored order IDs back to broker types
    fn parse_order_id(&self, order_id_str: &str) -> Result<Self::OrderId, Self::Error>;
}
