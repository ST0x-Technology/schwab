use sqlx::SqlitePool;
use tracing::info;

use super::trade::OnchainTrade;
use crate::onchain::{TradeConversionError, TradeStatus};
use crate::schwab::{SchwabInstruction, execution::SchwabExecution};

#[derive(Debug, Clone, PartialEq)]
pub struct TradeAccumulator {
    pub symbol: String,
    pub net_position: f64,
    pub accumulated_long: f64,  // Fractional shares accumulated for buying
    pub accumulated_short: f64, // Fractional shares accumulated for selling
    pub pending_execution_id: Option<i64>,
    pub threshold_amount: f64,
    pub last_updated: Option<String>,
}

impl TradeAccumulator {
    pub async fn add_trade(
        pool: &SqlitePool,
        trade: OnchainTrade,
        threshold: f64,
    ) -> Result<Option<SchwabExecution>, TradeConversionError> {
