/// Database persistence and data corruption errors.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Invalid direction in database: {0}")]
    InvalidDirection(#[from] crate::InvalidDirectionError),
    #[error("Invalid trade status in database: {0}")]
    InvalidTradeStatus(String),
    #[error("Invalid share quantity in database: {0}")]
    InvalidShareQuantity(i64),
    #[error("Invalid price cents in database: {0}")]
    InvalidPriceCents(i64),
    #[error("Execution missing ID after database save")]
    MissingExecutionId,
}

impl From<crate::BrokerError> for PersistenceError {
    fn from(err: crate::BrokerError) -> Self {
        match err {
            crate::BrokerError::Database(db_err) => Self::Database(db_err),
            other => Self::InvalidTradeStatus(format!("BrokerError: {}", other)),
        }
    }
}
