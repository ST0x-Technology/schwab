use super::position_calculator::PositionCalculator;
use crate::error::OnChainError;
use crate::onchain::TradeStatus;
use crate::schwab::{SchwabInstruction, execution::SchwabExecution};
use sqlx::SqlitePool;

/// Database repository for TradeAccumulator operations.
/// Separated from business logic to follow single responsibility principle.
pub struct TradeAccumulatorRepository;

impl TradeAccumulatorRepository {
    pub async fn get_or_create_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        symbol: &str,
    ) -> Result<PositionCalculator, OnChainError> {
        let row = sqlx::query!("SELECT * FROM trade_accumulators WHERE symbol = ?1", symbol)
            .fetch_optional(&mut **sql_tx)
            .await?;

        if let Some(row) = row {
            Ok(PositionCalculator::with_positions(
                row.net_position,
                row.accumulated_long,
                row.accumulated_short,
            ))
        } else {
            let new_calculator = PositionCalculator::new();
            Self::save_within_transaction(sql_tx, symbol, &new_calculator, None).await?;
            Ok(new_calculator)
        }
    }

    pub async fn save_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        symbol: &str,
        calculator: &PositionCalculator,
        pending_execution_id: Option<i64>,
    ) -> Result<(), OnChainError> {
        sqlx::query!(
            r#"
            INSERT OR REPLACE INTO trade_accumulators (
                symbol,
                net_position,
                accumulated_long,
                accumulated_short,
                pending_execution_id,
                last_updated
            )
            VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
            "#,
            symbol,
            calculator.net_position,
            calculator.accumulated_long,
            calculator.accumulated_short,
            pending_execution_id
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(())
    }

    pub async fn find_by_symbol(
        pool: &SqlitePool,
        symbol: &str,
    ) -> Result<Option<(PositionCalculator, Option<i64>)>, OnChainError> {
        let row = sqlx::query!("SELECT * FROM trade_accumulators WHERE symbol = ?1", symbol)
            .fetch_optional(pool)
            .await?;

        Ok(row.map(|row| {
            let calculator = PositionCalculator::with_positions(
                row.net_position,
                row.accumulated_long,
                row.accumulated_short,
            );
            (calculator, row.pending_execution_id)
        }))
    }

    pub async fn create_execution_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        symbol: &str,
        shares: u64,
        direction: SchwabInstruction,
    ) -> Result<SchwabExecution, OnChainError> {
        let execution = SchwabExecution {
            id: None,
            symbol: symbol.to_string(),
            shares,
            direction,
            status: TradeStatus::Pending,
        };

        let execution_id = execution.save_within_transaction(sql_tx).await?;
        let mut execution_with_id = execution;
        execution_with_id.id = Some(execution_id);

        Ok(execution_with_id)
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM trade_accumulators")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_get_or_create_new() {
        let pool = setup_test_db().await;
        let mut sql_tx = pool.begin().await.unwrap();

        let calculator =
            TradeAccumulatorRepository::get_or_create_within_transaction(&mut sql_tx, "AAPL")
                .await
                .unwrap();

        sql_tx.commit().await.unwrap();

        assert_eq!(calculator.net_position, 0.0);
        assert_eq!(calculator.accumulated_long, 0.0);
        assert_eq!(calculator.accumulated_short, 0.0);
    }

    #[tokio::test]
    async fn test_save_and_find() {
        let pool = setup_test_db().await;
        let mut sql_tx = pool.begin().await.unwrap();

        let calculator = PositionCalculator::with_positions(1.5, 2.0, 3.0);
        TradeAccumulatorRepository::save_within_transaction(
            &mut sql_tx,
            "MSFT",
            &calculator,
            None, // Don't use a pending execution ID for this test
        )
        .await
        .unwrap();

        sql_tx.commit().await.unwrap();

        let result = TradeAccumulatorRepository::find_by_symbol(&pool, "MSFT")
            .await
            .unwrap()
            .unwrap();

        let (found_calculator, pending_execution_id) = result;
        assert_eq!(found_calculator.net_position, 1.5);
        assert_eq!(found_calculator.accumulated_long, 2.0);
        assert_eq!(found_calculator.accumulated_short, 3.0);
        assert_eq!(pending_execution_id, None);
    }
}
