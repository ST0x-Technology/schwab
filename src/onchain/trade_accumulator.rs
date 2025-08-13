use sqlx::SqlitePool;
use tracing::info;

use super::trade::OnchainTrade;
use crate::onchain::{TradeConversionError, TradeStatus};
use crate::schwab::{SchwabInstruction, execution::SchwabExecution};

const SCHWAB_MINIMUM_WHOLE_SHARES: f64 = 1.0;

#[derive(Debug, Clone, Copy)]
enum ExecutionType {
    Long,
    Short,
}

/// Unified domain object that handles ALL fractional share accumulation logic.
/// This single object encapsulates:
/// - Position tracking (net position for threshold checking)  
/// - Trade accumulation (fractional amounts toward whole shares)
/// - Execution triggering (when ready, create Schwab execution)
/// - State transitions and database persistence
#[derive(Debug, Clone, PartialEq)]
pub struct TradeAccumulator {
    pub symbol: String,
    pub net_position: f64,
    pub accumulated_long: f64,  // Fractional shares accumulated for buying
    pub accumulated_short: f64, // Fractional shares accumulated for selling
    pub pending_execution_id: Option<i64>,
    pub last_updated: Option<String>,
}

impl TradeAccumulator {
    pub const fn new(symbol: String) -> Self {
        Self {
            symbol,
            net_position: 0.0,
            accumulated_long: 0.0,
            accumulated_short: 0.0,
            pending_execution_id: None,
            last_updated: None,
        }
    }

    /// The core method that handles everything: add a trade and potentially trigger execution.
    /// This method encapsulates all fractional share logic in one place.
    ///
    /// Returns Some(execution) if a Schwab execution was created, None otherwise.
    pub async fn add_trade(
        pool: &SqlitePool,
        trade: OnchainTrade,
    ) -> Result<Option<SchwabExecution>, TradeConversionError> {
        let mut sql_tx = pool.begin().await?;

        sql_tx.commit().await?;
        Ok(execution)
    }
