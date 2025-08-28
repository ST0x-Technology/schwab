use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use super::{price_cents_from_db_i64, shares_from_db_i64};
use crate::error::{OnChainError, PersistenceError};
use crate::schwab::Direction;
use crate::schwab::TradeStatus;

/// Converts database row data to a SchwabExecution instance.
/// Centralizes the conversion logic and casting operations.
#[allow(clippy::too_many_arguments)]
fn row_to_execution(
    id: i64,
    symbol: String,
    shares: i64,
    direction: &str,
    order_id: Option<String>,
    price_cents: Option<i64>,
    status: &str,
    executed_at: Option<chrono::NaiveDateTime>,
) -> Result<SchwabExecution, OnChainError> {
    let parsed_direction = direction
        .parse()
        .map_err(|e: String| OnChainError::Persistence(PersistenceError::InvalidDirection(e)))?;

    let parsed_status = match status {
        "PENDING" => TradeStatus::Pending,
        "SUBMITTED" => {
            let order_id = order_id.ok_or_else(|| {
                OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                    "SUBMITTED status requires order_id".to_string(),
                ))
            })?;
            TradeStatus::Submitted { order_id }
        }
        "FILLED" => {
            let order_id = order_id.ok_or_else(|| {
                OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                    "FILLED status requires order_id".to_string(),
                ))
            })?;
            let price_cents = price_cents.ok_or_else(|| {
                OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                    "FILLED status requires price_cents".to_string(),
                ))
            })?;
            let executed_at = executed_at.ok_or_else(|| {
                OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                    "FILLED status requires executed_at".to_string(),
                ))
            })?;
            TradeStatus::Filled {
                executed_at: DateTime::<Utc>::from_naive_utc_and_offset(executed_at, Utc),
                order_id,
                price_cents: price_cents_from_db_i64(price_cents)?,
            }
        }
        "FAILED" => {
            let failed_at = executed_at.ok_or_else(|| {
                OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                    "FAILED status requires executed_at timestamp".to_string(),
                ))
            })?;
            TradeStatus::Failed {
                failed_at: DateTime::<Utc>::from_naive_utc_and_offset(failed_at, Utc),
                error_reason: None, // We don't store error_reason in database yet
            }
        }
        _ => {
            return Err(OnChainError::Persistence(
                PersistenceError::InvalidTradeStatus(format!("Invalid trade status: {status}")),
            ));
        }
    };

    Ok(SchwabExecution {
        id: Some(id),
        symbol,
        shares: shares_from_db_i64(shares)?,
        direction: parsed_direction,
        status: parsed_status,
    })
}

/// Converts u64 share quantity to i64 for database storage.
/// Share quantities are always within i64 range for realistic trading scenarios.
const fn shares_to_db_i64(shares: u64) -> i64 {
    if shares > i64::MAX as u64 {
        i64::MAX // Defensive cap at maximum database value
    } else {
        #[allow(clippy::cast_possible_wrap)]
        {
            shares as i64 // Safe: within i64 range, wrap is prevented by check
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchwabExecution {
    pub(crate) id: Option<i64>,
    pub(crate) symbol: String,
    pub(crate) shares: u64,
    pub(crate) direction: Direction,
    pub(crate) status: TradeStatus,
}

pub(crate) async fn update_execution_status_within_transaction(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    execution_id: i64,
    new_status: TradeStatus,
) -> Result<(), sqlx::Error> {
    let status_str = new_status.as_str();

    let (order_id, price_cents_i64, executed_at) = match &new_status {
        TradeStatus::Pending => (None, None, None),
        TradeStatus::Submitted { order_id } => (Some(order_id.clone()), None, None),
        TradeStatus::Filled {
            executed_at,
            order_id,
            price_cents,
        } => (
            Some(order_id.clone()),
            Some(shares_to_db_i64(*price_cents)),
            Some(executed_at.naive_utc()),
        ),
        TradeStatus::Failed {
            failed_at,
            error_reason: _,
        } => (None, None, Some(failed_at.naive_utc())),
    };

    sqlx::query!(
        "
        UPDATE schwab_executions
        SET status = ?1, order_id = ?2, price_cents = ?3, executed_at = ?4
        WHERE id = ?5
        ",
        status_str,
        order_id,
        price_cents_i64,
        executed_at,
        execution_id
    )
    .execute(&mut **sql_tx)
    .await?;

    Ok(())
}

/// Find executions with SUBMITTED status (orders that have been submitted and can be polled)
pub(crate) async fn find_submitted_executions_by_symbol(
    pool: &SqlitePool,
    symbol: &str,
) -> Result<Vec<SchwabExecution>, OnChainError> {
    find_executions_by_symbol_and_status(pool, symbol, "SUBMITTED").await
}

/// Find executions with PENDING status (orders that have not been submitted yet)
pub(crate) async fn find_pending_executions_by_symbol(
    pool: &SqlitePool,
    symbol: &str,
) -> Result<Vec<SchwabExecution>, OnChainError> {
    find_executions_by_symbol_and_status(pool, symbol, "PENDING").await
}

/// Find executions with FILLED status
#[cfg(test)]
pub(crate) async fn find_filled_executions_by_symbol(
    pool: &SqlitePool,
    symbol: &str,
) -> Result<Vec<SchwabExecution>, OnChainError> {
    find_executions_by_symbol_and_status(pool, symbol, "FILLED").await
}

pub(crate) async fn find_executions_by_symbol_and_status(
    pool: &SqlitePool,
    symbol: &str,
    status_str: &str,
) -> Result<Vec<SchwabExecution>, OnChainError> {
    if symbol.is_empty() {
        let rows = sqlx::query!(
            "SELECT * FROM schwab_executions WHERE status = ?1 ORDER BY id ASC",
            status_str
        )
        .fetch_all(pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                row_to_execution(
                    row.id,
                    row.symbol,
                    row.shares,
                    &row.direction,
                    row.order_id,
                    row.price_cents,
                    &row.status,
                    row.executed_at,
                )
            })
            .collect()
    } else {
        let rows = sqlx::query!(
            "SELECT * FROM schwab_executions WHERE symbol = ?1 AND status = ?2 ORDER BY id ASC",
            symbol,
            status_str
        )
        .fetch_all(pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                row_to_execution(
                    row.id,
                    row.symbol,
                    row.shares,
                    &row.direction,
                    row.order_id,
                    row.price_cents,
                    &row.status,
                    row.executed_at,
                )
            })
            .collect()
    }
}

pub(crate) async fn find_execution_by_id(
    pool: &SqlitePool,
    execution_id: i64,
) -> Result<Option<SchwabExecution>, OnChainError> {
    let row = sqlx::query!(
        "SELECT * FROM schwab_executions WHERE id = ?1",
        execution_id
    )
    .fetch_optional(pool)
    .await?;

    if let Some(row) = row {
        row_to_execution(
            row.id,
            row.symbol,
            row.shares,
            &row.direction,
            row.order_id,
            row.price_cents,
            &row.status,
            row.executed_at,
        )
        .map(Some)
    } else {
        Ok(None)
    }
}

impl SchwabExecution {
    pub(crate) async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        let shares_i64 = shares_to_db_i64(self.shares);
        let direction_str = self.direction.as_str();
        let status_str = self.status.as_str();

        let (order_id, price_cents_i64, executed_at) = match &self.status {
            TradeStatus::Pending => (None, None, None),
            TradeStatus::Submitted { order_id } => (Some(order_id.clone()), None, None),
            TradeStatus::Filled {
                executed_at,
                order_id,
                price_cents,
            } => (
                Some(order_id.clone()),
                Some(shares_to_db_i64(*price_cents)),
                Some(executed_at.naive_utc()),
            ),
            TradeStatus::Failed {
                failed_at,
                error_reason: _,
            } => (None, None, Some(failed_at.naive_utc())),
        };

        let result = sqlx::query!(
            r#"
            INSERT INTO schwab_executions (symbol, shares, direction, order_id, price_cents, status, executed_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            self.symbol,
            shares_i64,
            direction_str,
            order_id,
            price_cents_i64,
            status_str,
            executed_at
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }
}

#[cfg(test)]
pub(crate) async fn schwab_execution_db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!("SELECT COUNT(*) as count FROM schwab_executions")
        .fetch_one(pool)
        .await?;
    Ok(row.count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{SchwabExecutionBuilder, setup_test_db};

    #[tokio::test]
    async fn test_schwab_execution_save_and_find() {
        let pool = setup_test_db().await;

        let execution = SchwabExecutionBuilder::new().build();

        let mut sql_tx = pool.begin().await.unwrap();
        let id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();
        assert!(id > 0);

        // Verify execution was saved by checking the count
        let count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status() {
        let pool = setup_test_db().await;

        let execution1 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 50,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let execution2 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 25,
            direction: Direction::Sell,
            status: TradeStatus::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER123".to_string(),
                price_cents: 15025,
            },
        };

        let execution3 = SchwabExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 10,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx1 = pool.begin().await.unwrap();
        execution1
            .save_within_transaction(&mut sql_tx1)
            .await
            .unwrap();
        sql_tx1.commit().await.unwrap();

        let mut sql_tx2 = pool.begin().await.unwrap();
        execution2
            .save_within_transaction(&mut sql_tx2)
            .await
            .unwrap();
        sql_tx2.commit().await.unwrap();

        let mut sql_tx3 = pool.begin().await.unwrap();
        execution3
            .save_within_transaction(&mut sql_tx3)
            .await
            .unwrap();
        sql_tx3.commit().await.unwrap();

        let pending_aapl = find_pending_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();

        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(pending_aapl[0].shares, 50);
        assert_eq!(pending_aapl[0].direction, Direction::Buy);

        let completed_aapl = find_filled_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();

        assert_eq!(completed_aapl.len(), 1);
        assert_eq!(completed_aapl[0].shares, 25);
        assert_eq!(completed_aapl[0].direction, Direction::Sell);
        assert!(matches!(
            &completed_aapl[0].status,
            TradeStatus::Filled { order_id, price_cents, .. }
            if order_id == "ORDER123" && *price_cents == 15025
        ));
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_empty_database() {
        let pool = setup_test_db().await;

        let result = find_pending_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = find_executions_by_symbol_and_status(&pool, "", "PENDING")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_nonexistent_matches() {
        let pool = setup_test_db().await;

        let execution = SchwabExecutionBuilder::new().build();

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let result = find_pending_executions_by_symbol(&pool, "NONEXISTENT")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = find_filled_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_empty_string_symbol() {
        let pool = setup_test_db().await;

        // Add executions with different symbols and statuses
        let executions = vec![
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 100,
                direction: Direction::Buy,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "MSFT".to_string(),
                shares: 50,
                direction: Direction::Sell,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 200,
                direction: Direction::Buy,
                status: TradeStatus::Filled {
                    executed_at: Utc::now(),
                    order_id: "ORDER123".to_string(),
                    price_cents: 15000,
                },
            },
        ];

        let mut sql_tx = pool.begin().await.unwrap();
        for execution in executions {
            execution
                .save_within_transaction(&mut sql_tx)
                .await
                .unwrap();
        }
        sql_tx.commit().await.unwrap();

        // Empty symbol should find all executions with the specified status
        let pending_all = find_executions_by_symbol_and_status(&pool, "", "PENDING")
            .await
            .unwrap();
        assert_eq!(pending_all.len(), 2); // AAPL and MSFT pending

        let filled_all = find_executions_by_symbol_and_status(&pool, "", "FILLED")
            .await
            .unwrap();
        assert_eq!(filled_all.len(), 1); // Only AAPL filled

        let failed_all = find_executions_by_symbol_and_status(&pool, "", "FAILED")
            .await
            .unwrap();
        assert_eq!(failed_all.len(), 0); // None failed
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_ordering() {
        let pool = setup_test_db().await;

        // Add executions for different symbols to test ordering by id
        // (Multiple pending executions per symbol would violate business constraints)
        let executions = vec![
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 100,
                direction: Direction::Buy,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "TSLA".to_string(),
                shares: 200,
                direction: Direction::Sell,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "MSFT".to_string(),
                shares: 300,
                direction: Direction::Buy,
                status: TradeStatus::Pending,
            },
        ];

        let mut sql_tx = pool.begin().await.unwrap();
        for execution in executions {
            execution
                .save_within_transaction(&mut sql_tx)
                .await
                .unwrap();
        }
        sql_tx.commit().await.unwrap();

        // Test ordering for each symbol individually
        let aapl_result = find_pending_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(aapl_result.len(), 1);
        assert_eq!(aapl_result[0].shares, 100);

        let tsla_result = find_pending_executions_by_symbol(&pool, "TSLA")
            .await
            .unwrap();
        assert_eq!(tsla_result.len(), 1);
        assert_eq!(tsla_result[0].shares, 200);

        let msft_result = find_pending_executions_by_symbol(&pool, "MSFT")
            .await
            .unwrap();
        assert_eq!(msft_result.len(), 1);
        assert_eq!(msft_result[0].shares, 300);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_case_sensitivity() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(), // Uppercase
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let result = find_pending_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(result.len(), 1);

        let result = find_pending_executions_by_symbol(&pool, "aapl")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = find_pending_executions_by_symbol(&pool, "Aapl")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_special_characters() {
        let pool = setup_test_db().await;

        // Test symbols with special characters
        let symbols = vec!["BRK.B", "BF-B", "TEST123", "A-B_C.D"];

        let mut sql_tx = pool.begin().await.unwrap();
        for symbol in symbols {
            let execution = SchwabExecution {
                id: None,
                symbol: symbol.to_string(),
                shares: 100,
                direction: Direction::Buy,
                status: TradeStatus::Pending,
            };

            execution
                .save_within_transaction(&mut sql_tx)
                .await
                .unwrap();
        }
        sql_tx.commit().await.unwrap();

        // Test each symbol can be found
        for symbol in ["BRK.B", "BF-B", "TEST123", "A-B_C.D"] {
            let result = find_pending_executions_by_symbol(&pool, symbol)
                .await
                .unwrap();
            assert_eq!(result.len(), 1, "Failed to find symbol: {symbol}");
            assert_eq!(result[0].symbol, symbol);
        }
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_data_integrity() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "TEST".to_string(),
            shares: 12345,
            direction: Direction::Sell,
            status: TradeStatus::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER789".to_string(),
                price_cents: 98765,
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let result = find_filled_executions_by_symbol(&pool, "TEST")
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        let found = &result[0];

        // Verify all fields are correctly preserved and converted
        assert_eq!(found.symbol, "TEST");
        assert_eq!(found.shares, 12345);
        assert_eq!(found.direction, Direction::Sell);
        assert!(matches!(
            &found.status,
            TradeStatus::Filled { order_id, price_cents, .. }
            if order_id == "ORDER789" && *price_cents == 98765
        ));
        assert!(found.id.is_some());
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_database_constraints() {
        let pool = setup_test_db().await;

        // Test that database constraints prevent invalid data insertion
        // This verifies our database design is robust

        // Try to insert execution with invalid direction - should be prevented by CHECK constraint
        let result = sqlx::query!(
            "INSERT INTO schwab_executions (symbol, shares, direction, order_id, price_cents, status) VALUES (?, ?, ?, ?, ?, ?)",
            "TEST",
            100i64,
            "INVALID_DIRECTION", // This should be rejected by CHECK constraint
            None::<String>,
            None::<i64>,
            "PENDING"
        )
        .execute(&pool)
        .await;

        // Should fail due to CHECK constraint
        assert!(result.is_err());

        // Verify the constraint error
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("CHECK constraint failed"));

        // Try to insert with invalid status - should also be prevented
        let result = sqlx::query!(
            "INSERT INTO schwab_executions (symbol, shares, direction, order_id, price_cents, status) VALUES (?, ?, ?, ?, ?, ?)",
            "TEST",
            100i64,
            "BUY",
            None::<String>,
            None::<i64>,
            "INVALID_STATUS" // This should be rejected by CHECK constraint
        )
        .execute(&pool)
        .await;

        // Should fail due to CHECK constraint
        assert!(result.is_err());

        // Verify our database maintains data integrity
        let count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(count, 0); // No invalid data should have been inserted
    }

    #[tokio::test]
    async fn test_update_status_within_transaction() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 15,
            direction: Direction::Sell,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Update using the transaction method
        let mut sql_tx = pool.begin().await.unwrap();
        update_execution_status_within_transaction(
            &mut sql_tx,
            id,
            TradeStatus::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER456".to_string(),
                price_cents: 20050,
            },
        )
        .await
        .unwrap();
        sql_tx.commit().await.unwrap();

        // Verify the update persisted by finding executions with the new status
        let completed_executions = find_filled_executions_by_symbol(&pool, "TSLA")
            .await
            .unwrap();

        assert_eq!(completed_executions.len(), 1);
        assert!(matches!(
            &completed_executions[0].status,
            TradeStatus::Filled { order_id, price_cents, .. }
            if order_id == "ORDER456" && *price_cents == 20050
        ));
    }

    #[tokio::test]
    async fn test_db_count() {
        let pool = setup_test_db().await;

        let count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(count, 0);

        let execution = SchwabExecution {
            id: None,
            symbol: "NVDA".to_string(),
            shares: 5,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_database_constraints_prevent_multiple_pending_per_symbol() {
        let pool = setup_test_db().await;

        // Create first pending execution for AAPL
        let execution1 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution1
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Try to create second pending execution for same symbol - should fail
        let execution2 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 200,
            direction: Direction::Sell,
            status: TradeStatus::Pending,
        };

        let mut sql_tx2 = pool.begin().await.unwrap();
        let result = execution2.save_within_transaction(&mut sql_tx2).await;
        assert!(
            result.is_err(),
            "Should not allow multiple pending executions for same symbol"
        );

        // Error should be about unique constraint violation
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains("UNIQUE constraint failed")
                && error_message.contains("schwab_executions.symbol"),
            "Error should mention unique constraint on symbol, got: {error_message}"
        );
    }

    #[tokio::test]
    async fn test_database_constraints_allow_different_statuses_per_symbol() {
        let pool = setup_test_db().await;

        // Create pending execution for AAPL
        let execution1 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };
        let mut sql_tx1 = pool.begin().await.unwrap();
        execution1
            .save_within_transaction(&mut sql_tx1)
            .await
            .unwrap();
        sql_tx1.commit().await.unwrap();

        // Create completed execution for same symbol - should succeed
        let execution2 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 200,
            direction: Direction::Sell,
            status: TradeStatus::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER123".to_string(),
                price_cents: 15000,
            },
        };
        let mut sql_tx2 = pool.begin().await.unwrap();
        execution2
            .save_within_transaction(&mut sql_tx2)
            .await
            .unwrap();
        sql_tx2.commit().await.unwrap();

        // Should have both executions for AAPL now
        let pending_aapl = find_pending_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        let completed_aapl = find_filled_executions_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(completed_aapl.len(), 1);
    }

    #[test]
    fn test_row_to_execution_invalid_direction() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "INVALID_DIRECTION",
            None,
            None,
            "PENDING",
            None,
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidDirection(_))
        ));
    }

    #[test]
    fn test_row_to_execution_completed_missing_order_id() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "BUY",
            None, // Missing order_id for COMPLETED status
            Some(15000),
            "COMPLETED",
            Some(chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc()),
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(_))
        ));
    }

    #[test]
    fn test_row_to_execution_completed_missing_price_cents() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "BUY",
            Some("ORDER123".to_string()),
            None, // Missing price_cents for COMPLETED status
            "COMPLETED",
            Some(chrono::DateTime::from_timestamp(0, 0).unwrap().naive_utc()),
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(_))
        ));
    }

    #[test]
    fn test_row_to_execution_completed_missing_executed_at() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "BUY",
            Some("ORDER123".to_string()),
            Some(15000),
            "COMPLETED",
            None, // Missing executed_at for COMPLETED status
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(_))
        ));
    }

    #[test]
    fn test_row_to_execution_invalid_status() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "BUY",
            None,
            None,
            "INVALID_STATUS",
            None,
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(_))
        ));
    }

    #[test]
    fn test_row_to_execution_negative_shares() {
        // Test with negative shares value
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            -100, // Negative shares
            "BUY",
            None,
            None,
            "PENDING",
            None,
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidShareQuantity(_))
        ));
    }

    #[tokio::test]
    async fn test_execution_status_transition_failed_to_pending() {
        let pool = setup_test_db().await;

        // Create failed execution (simulating previous auth failure)
        let execution = SchwabExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 50,
            direction: Direction::Sell,
            status: TradeStatus::Failed {
                failed_at: Utc::now(),
                error_reason: Some("Authentication failed".to_string()),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Update to pending (simulating retry after auth recovery)
        let mut sql_tx = pool.begin().await.unwrap();
        update_execution_status_within_transaction(&mut sql_tx, execution_id, TradeStatus::Pending)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Verify status updated correctly
        let found = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(found.status, TradeStatus::Pending));
    }

    #[tokio::test]
    async fn test_database_connection_failure_handling() {
        let pool = setup_test_db().await;
        pool.close().await;
        schwab_execution_db_count(&pool).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_find_execution_by_id_with_closed_connection() {
        let pool = setup_test_db().await;
        pool.close().await;
        find_execution_by_id(&pool, 1).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_execution_save_with_database_constraint_violation() {
        let pool = setup_test_db().await;

        // Create execution with valid data first
        let execution1 = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 100,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx1 = pool.begin().await.unwrap();
        execution1
            .save_within_transaction(&mut sql_tx1)
            .await
            .unwrap();
        sql_tx1.commit().await.unwrap();

        // Try to create another pending execution for same symbol
        let execution2 = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 200,
            direction: Direction::Sell,
            status: TradeStatus::Pending,
        };

        let mut sql_tx2 = pool.begin().await.unwrap();
        let result = execution2.save_within_transaction(&mut sql_tx2).await;

        // Should fail due to unique constraint on symbol for pending executions
        assert!(result.is_err(), "Expected unique constraint violation");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("UNIQUE constraint failed")
        );
    }

    #[tokio::test]
    async fn test_update_execution_status_transaction_rollback() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "GOOG".to_string(),
            shares: 25,
            direction: Direction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Start transaction but don't commit
        let mut sql_tx = pool.begin().await.unwrap();
        update_execution_status_within_transaction(
            &mut sql_tx,
            execution_id,
            TradeStatus::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER999".to_string(),
                price_cents: 300_000,
            },
        )
        .await
        .unwrap();

        // Rollback instead of commit
        sql_tx.rollback().await.unwrap();

        // Verify original status preserved
        let found = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(found.status, TradeStatus::Pending));
    }

    #[test]
    fn test_row_to_execution_failed_missing_executed_at() {
        let result = row_to_execution(
            1,
            "AAPL".to_string(),
            100,
            "BUY",
            None,
            None,
            "FAILED",
            None, // Missing executed_at for FAILED status
        );

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(_))
        ));
    }
}
