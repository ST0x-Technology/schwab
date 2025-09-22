use std::num::ParseFloatError;

/// Database persistence and data corruption errors.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Invalid direction in database: {0}")]
    InvalidDirection(String),
    #[error("Invalid trade status in database: {0}")]
    InvalidTradeStatus(String),
    #[error("Invalid share quantity in database: {0}")]
    InvalidShareQuantity(i64),
    #[error("Invalid price cents in database: {0}")]
    InvalidPriceCents(i64),
    #[error("Execution missing ID after database save")]
    MissingExecutionId,
}

/// Simplified error type for broker operations
#[derive(Debug, thiserror::Error)]
pub enum OnChainError {
    #[error("Database persistence error: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("Failed to convert U256 to f64: {0}")]
    U256ToF64(#[from] ParseFloatError),
}

impl From<sqlx::Error> for OnChainError {
    fn from(err: sqlx::Error) -> Self {
        Self::Persistence(PersistenceError::Database(err))
    }
}
