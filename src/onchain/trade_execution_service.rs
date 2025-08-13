use sqlx::SqlitePool;
use tracing::info;

use super::{
    position_calculator::{ExecutionType, PositionCalculator},
    trade::OnchainTrade,
    trade_accumulator_repository::TradeAccumulatorRepository,
};
use crate::error::OnChainError;
use crate::schwab::{SchwabInstruction, execution::SchwabExecution};

/// Service that coordinates trade execution between components.
/// Orchestrates interactions between PositionCalculator and TradeAccumulatorRepository.
pub struct TradeExecutionService;

impl TradeExecutionService {
    pub async fn add_trade(
        pool: &SqlitePool,
        trade: OnchainTrade,
    ) -> Result<Option<SchwabExecution>, OnChainError> {
        let mut sql_tx = pool.begin().await?;

        let trade_id = trade.save_within_transaction(&mut sql_tx).await?;
        info!(
            "Saved onchain trade: id={}, symbol={}, amount={}",
            trade_id, trade.symbol, trade.amount
        );

        let base_symbol = Self::extract_base_symbol(&trade.symbol)?;

        let mut calculator =
            TradeAccumulatorRepository::get_or_create_within_transaction(&mut sql_tx, &base_symbol)
                .await?;

        calculator.add_trade_amount(trade.amount);

        info!(
            "Updated calculator for {}: net_position={}, accumulated_long={}, accumulated_short={}",
            base_symbol,
            calculator.net_position,
            calculator.accumulated_long,
            calculator.accumulated_short
        );

        let execution = Self::try_create_execution_if_ready(
            &mut sql_tx,
            &base_symbol,
            trade_id,
            &mut calculator,
        )
        .await?;

        TradeAccumulatorRepository::save_within_transaction(
            &mut sql_tx,
            &base_symbol,
            &calculator,
            None,
        )
        .await?;

        sql_tx.commit().await?;
        Ok(execution)
    }

    fn extract_base_symbol(symbol: &str) -> Result<String, OnChainError> {
        if !symbol.ends_with("s1") {
            return Err(OnChainError::InvalidSymbolConfiguration(
                symbol.to_string(),
                "TradeAccumulator only processes tokenized equity symbols (s1 suffix)".to_string(),
            ));
        }

        symbol
            .strip_suffix("s1")
            .map(ToString::to_string)
            .ok_or_else(|| {
                OnChainError::InvalidSymbolConfiguration(
                    symbol.to_string(),
                    "Failed to extract base symbol from s1 suffix".to_string(),
                )
            })
    }

    async fn try_create_execution_if_ready(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        base_symbol: &str,
        trade_id: i64,
        calculator: &mut PositionCalculator,
    ) -> Result<Option<SchwabExecution>, OnChainError> {
        let Some(execution_type) = calculator.determine_execution_type() else {
            return Ok(None);
        };

        Self::execute_position(sql_tx, base_symbol, trade_id, calculator, execution_type).await
    }

    async fn execute_position(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        base_symbol: &str,
        _trade_id: i64,
        calculator: &mut PositionCalculator,
        execution_type: ExecutionType,
    ) -> Result<Option<SchwabExecution>, OnChainError> {
        let shares = calculator.calculate_executable_shares(execution_type);

        if shares == 0 {
            return Ok(None);
        }

        let instruction = match execution_type {
            ExecutionType::Long => SchwabInstruction::Buy,
            ExecutionType::Short => SchwabInstruction::Sell,
        };

        let execution = TradeAccumulatorRepository::create_execution_within_transaction(
            sql_tx,
            base_symbol,
            shares,
            instruction,
        )
        .await?;

        calculator.reduce_accumulation(execution_type, shares);

        info!(
            "Created Schwab execution: symbol={}, shares={}, direction={:?}",
            base_symbol, shares, instruction
        );

        Ok(Some(execution))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::fixed_bytes;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_add_trade_below_threshold() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 0.5,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = TradeExecutionService::add_trade(&pool, trade)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_add_trade_above_threshold() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x2222222222222222222222222222222222222222222222222222222222222222"
            ),
            log_index: 1,
            symbol: "MSFTs1".to_string(),
            amount: 1.5,
            price_usdc: 300.0,
            created_at: None,
        };

        let result = TradeExecutionService::add_trade(&pool, trade)
            .await
            .unwrap();
        let execution = result.unwrap();

        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, SchwabInstruction::Sell);
    }

    #[tokio::test]
    async fn test_extract_base_symbol() {
        assert_eq!(
            TradeExecutionService::extract_base_symbol("AAPLs1").unwrap(),
            "AAPL"
        );

        let result = TradeExecutionService::extract_base_symbol("INVALID");
        assert!(result.is_err());
    }
}
