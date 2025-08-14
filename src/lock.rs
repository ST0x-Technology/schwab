use crate::error::OnChainError;

/// Atomically acquires an execution lease for the given symbol.
/// Returns true if lease was acquired, false if another worker holds it.
pub async fn try_acquire_execution_lease(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
) -> Result<bool, OnChainError> {
    // Create symbol_locks table if it doesn't exist (for tests)
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS symbol_locks (
            symbol TEXT PRIMARY KEY NOT NULL,
            locked_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
        )
        ",
    )
    .execute(sql_tx.as_mut())
    .await?;

    // Clean up old locks first (older than 5 minutes)
    sqlx::query("DELETE FROM symbol_locks WHERE locked_at < datetime('now', '-5 minutes')")
        .execute(sql_tx.as_mut())
        .await?;

    // Try to acquire lock by inserting into symbol_locks table
    let result = sqlx::query("INSERT OR IGNORE INTO symbol_locks (symbol) VALUES (?1)")
        .bind(symbol)
        .execute(sql_tx.as_mut())
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Clears the execution lease when no execution was created
pub async fn clear_execution_lease(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
) -> Result<(), OnChainError> {
    sqlx::query("DELETE FROM symbol_locks WHERE symbol = ?1")
        .bind(symbol)
        .execute(sql_tx.as_mut())
        .await?;

    Ok(())
}

/// Sets the actual execution ID after successful execution creation
pub async fn set_pending_execution_id(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
    execution_id: i64,
) -> Result<(), OnChainError> {
    sqlx::query!(
        r#"
        UPDATE trade_accumulators 
        SET pending_execution_id = ?1, last_updated = CURRENT_TIMESTAMP
        WHERE symbol = ?2
        "#,
        execution_id,
        symbol
    )
    .execute(sql_tx.as_mut())
    .await?;

    Ok(())
}

/// Clears pending execution within a transaction
pub async fn clear_pending_execution_within_transaction(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
    execution_id: i64,
) -> Result<(), OnChainError> {
    // Clear the pending_execution_id in the accumulator
    sqlx::query!(
        r#"
        UPDATE trade_accumulators 
        SET pending_execution_id = NULL, last_updated = CURRENT_TIMESTAMP
        WHERE symbol = ?1 AND pending_execution_id = ?2
        "#,
        symbol,
        execution_id
    )
    .execute(sql_tx.as_mut())
    .await?;

    // Release the symbol lock
    sqlx::query("DELETE FROM symbol_locks WHERE symbol = ?1")
        .bind(symbol)
        .execute(sql_tx.as_mut())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::position_calculator::PositionCalculator;
    use crate::schwab::{Direction, TradeStatus, execution::SchwabExecution};
    use crate::test_utils::setup_test_db;

    async fn save_within_transaction(
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
        .execute(sql_tx.as_mut())
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_try_acquire_execution_lease_success() {
        let pool = setup_test_db().await;
        let mut sql_tx = pool.begin().await.unwrap();

        let result = try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        assert!(result);

        sql_tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn test_try_acquire_execution_lease_conflict() {
        let pool = setup_test_db().await;

        // First transaction acquires the lease
        let mut sql_tx1 = pool.begin().await.unwrap();
        let result1 = try_acquire_execution_lease(&mut sql_tx1, "AAPL")
            .await
            .unwrap();
        assert!(result1);
        sql_tx1.commit().await.unwrap();

        // Second transaction tries to acquire the same lease and should fail
        let mut sql_tx2 = pool.begin().await.unwrap();
        let result2 = try_acquire_execution_lease(&mut sql_tx2, "AAPL")
            .await
            .unwrap();
        assert!(!result2);
        sql_tx2.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn test_try_acquire_execution_lease_different_symbols() {
        let pool = setup_test_db().await;

        // Acquire lease for first symbol
        let mut sql_tx1 = pool.begin().await.unwrap();
        let result1 = try_acquire_execution_lease(&mut sql_tx1, "AAPL")
            .await
            .unwrap();
        assert!(result1);
        sql_tx1.commit().await.unwrap();

        // Acquire lease for different symbol (should succeed)
        let mut sql_tx2 = pool.begin().await.unwrap();
        let result2 = try_acquire_execution_lease(&mut sql_tx2, "MSFT")
            .await
            .unwrap();
        assert!(result2);
        sql_tx2.commit().await.unwrap();
    }

    #[tokio::test]
    async fn test_clear_execution_lease() {
        let pool = setup_test_db().await;

        let mut sql_tx1 = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx1, "AAPL")
            .await
            .unwrap();
        assert!(result);
        sql_tx1.commit().await.unwrap();

        let mut sql_tx2 = pool.begin().await.unwrap();
        clear_execution_lease(&mut sql_tx2, "AAPL").await.unwrap();
        sql_tx2.commit().await.unwrap();

        let mut sql_tx3 = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx3, "AAPL")
            .await
            .unwrap();
        assert!(result);
        sql_tx3.commit().await.unwrap();
    }

    #[tokio::test]
    async fn test_clear_pending_execution() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        let calculator = PositionCalculator::new();
        save_within_transaction(&mut sql_tx, "AAPL", &calculator, Some(execution_id))
            .await
            .unwrap();

        try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let mut sql_tx = pool.begin().await.unwrap();
        clear_pending_execution_within_transaction(&mut sql_tx, "AAPL", execution_id)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Should be able to acquire lease again (verifying lock was released)
        let mut sql_tx = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        assert!(result);
        sql_tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn test_clear_pending_execution_within_transaction() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        let calculator = PositionCalculator::new();
        save_within_transaction(&mut sql_tx, "AAPL", &calculator, Some(execution_id))
            .await
            .unwrap();

        try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();

        clear_pending_execution_within_transaction(&mut sql_tx, "AAPL", execution_id)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Should be able to acquire lease again (verifying lock was released)
        let mut sql_tx = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        assert!(result);
        sql_tx.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn test_symbol_locks_table_creation() {
        let pool = setup_test_db().await;

        let mut sql_tx = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx, "TEST")
            .await
            .unwrap();
        assert!(result);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM symbol_locks")
            .fetch_one(sql_tx.as_mut())
            .await
            .unwrap();
        assert_eq!(count, 1);

        sql_tx.commit().await.unwrap();
    }
}
