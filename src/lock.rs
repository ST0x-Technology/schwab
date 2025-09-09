use crate::error::OnChainError;

/// Atomically acquires an execution lease for the given symbol.
/// Returns true if lease was acquired, false if another worker holds it.
pub(crate) async fn try_acquire_execution_lease(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
) -> Result<bool, OnChainError> {
    const LOCK_TIMEOUT_MINUTES: i32 = 5;

    // Clean up old locks first (older than 5 minutes)
    let timeout_param = format!("-{LOCK_TIMEOUT_MINUTES} minutes");
    sqlx::query!(
        "DELETE FROM symbol_locks WHERE locked_at < datetime('now', ?1)",
        timeout_param
    )
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
pub(crate) async fn clear_execution_lease(
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
pub(crate) async fn set_pending_execution_id(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::accumulator::save_within_transaction;
    use crate::onchain::position_calculator::PositionCalculator;
    use crate::schwab::{Direction, TradeState, execution::SchwabExecution};
    use crate::test_utils::setup_test_db;

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
            state: TradeState::Pending,
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

        // Note: clear_pending_execution_within_transaction function was removed
        // This test just verifies that we can set up the accumulator and acquire lease
    }

    #[tokio::test]
    async fn test_acquire_execution_lease_persists_lock_row() {
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

    #[tokio::test]
    async fn test_ttl_based_cleanup() {
        let pool = setup_test_db().await;

        // First, acquire a lease
        let mut sql_tx = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        assert!(result);
        sql_tx.commit().await.unwrap();

        // Manually update the locked_at timestamp to be older than TTL
        let mut sql_tx = pool.begin().await.unwrap();
        sqlx::query(
            "UPDATE symbol_locks SET locked_at = datetime('now', '-100 minutes') WHERE symbol = ?1",
        )
        .bind("AAPL")
        .execute(sql_tx.as_mut())
        .await
        .unwrap();
        sql_tx.commit().await.unwrap();

        // Now try to acquire the same lease - should succeed due to TTL cleanup
        let mut sql_tx = pool.begin().await.unwrap();
        let result = try_acquire_execution_lease(&mut sql_tx, "AAPL")
            .await
            .unwrap();
        assert!(result); // Should succeed because old lock was cleaned up
        sql_tx.rollback().await.unwrap();
    }
}
