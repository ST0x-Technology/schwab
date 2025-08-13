use sqlx::SqlitePool;

use super::{
    position_calculator::PositionCalculator, trade::OnchainTrade,
    trade_accumulator_repository::TradeAccumulatorRepository,
    trade_execution_service::TradeExecutionService,
};
use crate::error::OnChainError;
use crate::schwab::execution::SchwabExecution;

/// Entry point for trade accumulation operations.
/// Provides a clean interface to the underlying service layer.
pub struct TradeAccumulator;

impl TradeAccumulator {
    pub async fn add_trade(
        pool: &SqlitePool,
        trade: OnchainTrade,
    ) -> Result<Option<SchwabExecution>, OnChainError> {
        TradeExecutionService::add_trade(pool, trade).await
    }

    pub async fn find_by_symbol(
        pool: &SqlitePool,
        symbol: &str,
    ) -> Result<Option<(PositionCalculator, Option<i64>)>, OnChainError> {
        TradeAccumulatorRepository::find_by_symbol(pool, symbol).await
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        TradeAccumulatorRepository::db_count(pool).await
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

        let result = TradeAccumulator::add_trade(&pool, trade).await.unwrap();
        assert!(result.is_none());

        let (calculator, _) = TradeAccumulator::find_by_symbol(&pool, "AAPL")
            .await
            .unwrap()
            .unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON);
        assert!((calculator.net_position - 0.5).abs() < f64::EPSILON);
        assert!((calculator.accumulated_long - 0.0).abs() < f64::EPSILON);
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

        let result = TradeAccumulator::add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, crate::schwab::SchwabInstruction::Sell);
        assert_eq!(execution.status, crate::onchain::TradeStatus::Pending);

        let (calculator, _) = TradeAccumulator::find_by_symbol(&pool, "MSFT")
            .await
            .unwrap()
            .unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON);
        assert!((calculator.net_position - 1.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_accumulation_across_multiple_trades() {
        let pool = setup_test_db().await;

        let trade1 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x3333333333333333333333333333333333333333333333333333333333333333"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 0.3,
            price_usdc: 150.0,
            created_at: None,
        };

        let result1 = TradeAccumulator::add_trade(&pool, trade1).await.unwrap();
        assert!(result1.is_none());

        let trade2 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x4444444444444444444444444444444444444444444444444444444444444444"
            ),
            log_index: 2,
            symbol: "AAPLs1".to_string(),
            amount: 0.4,
            price_usdc: 150.0,
            created_at: None,
        };

        let result2 = TradeAccumulator::add_trade(&pool, trade2).await.unwrap();
        assert!(result2.is_none());

        let trade3 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x5555555555555555555555555555555555555555555555555555555555555555"
            ),
            log_index: 3,
            symbol: "AAPLs1".to_string(),
            amount: 0.4,
            price_usdc: 150.0,
            created_at: None,
        };

        let result3 = TradeAccumulator::add_trade(&pool, trade3).await.unwrap();
        let execution = result3.unwrap();

        assert_eq!(execution.symbol, "AAPL");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, crate::schwab::SchwabInstruction::Sell);

        let (calculator, _) = TradeAccumulator::find_by_symbol(&pool, "AAPL")
            .await
            .unwrap()
            .unwrap();
        assert!((calculator.accumulated_short - 0.1).abs() < f64::EPSILON);
        assert!((calculator.net_position - 1.1).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_add_trade_invalid_symbol_rejects() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x6666666666666666666666666666666666666666666666666666666666666666"
            ),
            log_index: 1,
            symbol: "INVALID".to_string(),
            amount: 1.0,
            price_usdc: 100.0,
            created_at: None,
        };

        let result = TradeAccumulator::add_trade(&pool, trade).await;
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(
                crate::error::TradeValidationError::InvalidSymbolConfiguration(_, _)
            )
        ));
    }

    #[tokio::test]
    async fn test_add_trade_usdc_symbol_rejected() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x7777777777777777777777777777777777777777777777777777777777777777"
            ),
            log_index: 1,
            symbol: "USDC".to_string(),
            amount: 100.0,
            price_usdc: 1.0,
            created_at: None,
        };

        let result = TradeAccumulator::add_trade(&pool, trade).await;
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(
                crate::error::TradeValidationError::InvalidSymbolConfiguration(_, _)
            )
        ));
    }
}
