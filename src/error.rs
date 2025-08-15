//! Domain-specific error types following clean error handling architecture.
//! Separates concerns instead of mixing database, business logic, and external API errors.

use alloy::primitives::{B256, ruint::FromUintError};
use alloy::transports::{RpcError, TransportErrorKind};
use std::num::ParseFloatError;

/// Business logic validation errors for trade processing rules.
#[derive(Debug, thiserror::Error)]
pub enum TradeValidationError {
    #[error("No transaction hash found in log")]
    NoTxHash,
    #[error("No log index found in log")]
    NoLogIndex,
    #[error("No block number found in log")]
    NoBlockNumber,
    #[error("Invalid IO index: {0}")]
    InvalidIndex(#[from] FromUintError<usize>),
    #[error("No input found at index: {0}")]
    NoInputAtIndex(usize),
    #[error("No output found at index: {0}")]
    NoOutputAtIndex(usize),
    #[error("Expected IO to contain USDC and one s1-suffixed symbol but got {0} and {1}")]
    InvalidSymbolConfiguration(String, String),
    #[error("Failed to convert U256 to f64: {0}")]
    U256ToF64(#[from] ParseFloatError),
    #[error("Transaction not found: {0}")]
    TransactionNotFound(B256),
    #[error("No AfterClear log found for ClearV2 log")]
    NoAfterClearLog,
}

/// Database persistence and data corruption errors.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Invalid Schwab instruction in database: {0}")]
    InvalidSchwabInstruction(String),
    #[error("Invalid trade status in database: {0}")]
    InvalidTradeStatus(String),
    #[error("Invalid share quantity in database: {0}")]
    InvalidShareQuantity(i64),
    #[error("Failed to acquire symbol map lock")]
    SymbolMapLock,
    #[error("Execution ID mismatch for symbol {symbol}: expected {expected}, current {current:?}")]
    ExecutionIdMismatch {
        symbol: String,
        expected: i64,
        current: Option<i64>,
    },
    #[error("Execution missing ID after database save")]
    MissingExecutionId,
}

/// External service and API interaction errors.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Failed to get symbol: {0}")]
    GetSymbol(#[from] alloy::contract::Error),
    #[error("Sol type error: {0}")]
    SolType(#[from] alloy::sol_types::Error),
    #[error("RPC transport error: {0}")]
    RpcTransport(#[from] RpcError<TransportErrorKind>),
    #[error("Schwab API error: {0}")]
    SchwabApi(String),
}

/// Unified error type for onchain trade processing with clear domain boundaries.
/// Provides error mapping between layers while maintaining separation of concerns.
#[derive(Debug, thiserror::Error)]
pub enum OnChainError {
    #[error("Trade validation error: {0}")]
    Validation(#[from] TradeValidationError),
    #[error("Database persistence error: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("External execution error: {0}")]
    Execution(#[from] ExecutionError),
}

impl From<sqlx::Error> for OnChainError {
    fn from(err: sqlx::Error) -> Self {
        Self::Persistence(PersistenceError::Database(err))
    }
}

impl From<alloy::contract::Error> for OnChainError {
    fn from(err: alloy::contract::Error) -> Self {
        Self::Execution(ExecutionError::GetSymbol(err))
    }
}

impl From<ParseFloatError> for OnChainError {
    fn from(err: ParseFloatError) -> Self {
        Self::Validation(TradeValidationError::U256ToF64(err))
    }
}

impl From<FromUintError<usize>> for OnChainError {
    fn from(err: FromUintError<usize>) -> Self {
        Self::Validation(TradeValidationError::InvalidIndex(err))
    }
}

impl From<alloy::sol_types::Error> for OnChainError {
    fn from(err: alloy::sol_types::Error) -> Self {
        Self::Execution(ExecutionError::SolType(err))
    }
}

impl From<RpcError<TransportErrorKind>> for OnChainError {
    fn from(err: RpcError<TransportErrorKind>) -> Self {
        Self::Execution(ExecutionError::RpcTransport(err))
    }
}
