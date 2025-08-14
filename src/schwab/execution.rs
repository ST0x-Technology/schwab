use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::error::OnChainError;
use crate::schwab::SchwabInstruction;
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
        .map_err(OnChainError::InvalidSchwabInstruction)?;

    let parsed_status = match status {
        "PENDING" => TradeStatus::Pending,
        "COMPLETED" => {
            let order_id = order_id.ok_or_else(|| {
                OnChainError::InvalidTradeStatus("COMPLETED status requires order_id".to_string())
            })?;
            let price_cents = price_cents.ok_or_else(|| {
                OnChainError::InvalidTradeStatus(
                    "COMPLETED status requires price_cents".to_string(),
                )
            })?;
            let executed_at = executed_at.ok_or_else(|| {
                OnChainError::InvalidTradeStatus(
                    "COMPLETED status requires executed_at".to_string(),
                )
            })?;
            TradeStatus::Completed {
                executed_at: DateTime::<Utc>::from_naive_utc_and_offset(executed_at, Utc),
                order_id,
                price_cents: shares_from_db_i64(price_cents),
            }
        }
        "FAILED" => {
            let failed_at = executed_at.ok_or_else(|| {
                OnChainError::InvalidTradeStatus(
                    "FAILED status requires executed_at timestamp".to_string(),
                )
            })?;
            TradeStatus::Failed {
                failed_at: DateTime::<Utc>::from_naive_utc_and_offset(failed_at, Utc),
                error_reason: None, // We don't store error_reason in database yet
            }
        }
        _ => {
            return Err(OnChainError::InvalidTradeStatus(format!(
                "Invalid trade status: {status}"
            )));
        }
    };

    Ok(SchwabExecution {
        id: Some(id),
        symbol,
        shares: shares_from_db_i64(shares),
        direction: parsed_direction,
        status: parsed_status,
    })
}

/// Converts database i64 to u64 for share quantities.
/// Database stores as i64 but shares are always positive quantities.
const fn shares_from_db_i64(db_value: i64) -> u64 {
    if db_value < 0 {
        0 // Defensive programming: negative shares shouldn't exist in our domain
    } else {
        #[allow(clippy::cast_sign_loss)]
        {
            db_value as u64 // Safe: non-negative value, sign loss is intentional
        }
    }
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
pub struct SchwabExecution {
    pub id: Option<i64>,
    pub symbol: String,
    pub shares: u64,
    pub direction: SchwabInstruction,
    pub status: TradeStatus,
}

impl SchwabExecution {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        let shares_i64 = shares_to_db_i64(self.shares);
        let direction_str = self.direction.as_str();
        let status_str = self.status.as_str();

        let (order_id, price_cents_i64, executed_at) = match &self.status {
            TradeStatus::Pending => (None, None, None),
            TradeStatus::Completed {
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

    /// Find executions with PENDING status
    pub async fn find_pending_by_symbol(
        pool: &SqlitePool,
        symbol: &str,
    ) -> Result<Vec<Self>, OnChainError> {
        Self::find_by_symbol_and_status(pool, symbol, "PENDING").await
    }

    /// Find executions with COMPLETED status
    pub async fn find_completed_by_symbol(
        pool: &SqlitePool,
        symbol: &str,
    ) -> Result<Vec<Self>, OnChainError> {
        Self::find_by_symbol_and_status(pool, symbol, "COMPLETED").await
    }

    /// Find executions with FAILED status
    pub async fn find_failed_by_symbol(
        pool: &SqlitePool,
        symbol: &str,
    ) -> Result<Vec<Self>, OnChainError> {
        Self::find_by_symbol_and_status(pool, symbol, "FAILED").await
    }

    pub async fn find_by_id(
        pool: &SqlitePool,
        execution_id: i64,
    ) -> Result<Option<Self>, OnChainError> {
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

    pub async fn find_by_symbol_and_status(
        pool: &SqlitePool,
        symbol: &str,
        status_str: &str,
    ) -> Result<Vec<Self>, OnChainError> {
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

    pub async fn update_status_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        execution_id: i64,
        new_status: TradeStatus,
    ) -> Result<(), sqlx::Error> {
        let status_str = new_status.as_str();

        let (order_id, price_cents_i64, executed_at) = match &new_status {
            TradeStatus::Pending => (None, None, None),
            TradeStatus::Completed {
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
            "UPDATE schwab_executions SET status = ?1, order_id = ?2, price_cents = ?3, executed_at = ?4 WHERE id = ?5",
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

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM schwab_executions")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }
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
        let count = SchwabExecution::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status() {
        let pool = setup_test_db().await;

        let execution1 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 50,
            direction: SchwabInstruction::Buy,
            status: TradeStatus::Pending,
        };

        let execution2 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 25,
            direction: SchwabInstruction::Sell,
            status: TradeStatus::Completed {
                executed_at: Utc::now(),
                order_id: "ORDER123".to_string(),
                price_cents: 15025,
            },
        };

        let execution3 = SchwabExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 10,
            direction: SchwabInstruction::Buy,
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

        let pending_aapl = SchwabExecution::find_pending_by_symbol(&pool, "AAPL")
            .await
            .unwrap();

        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(pending_aapl[0].shares, 50);
        assert_eq!(pending_aapl[0].direction, SchwabInstruction::Buy);

        let completed_aapl = SchwabExecution::find_completed_by_symbol(&pool, "AAPL")
            .await
            .unwrap();

        assert_eq!(completed_aapl.len(), 1);
        assert_eq!(completed_aapl[0].shares, 25);
        assert_eq!(completed_aapl[0].direction, SchwabInstruction::Sell);
        assert!(matches!(
            &completed_aapl[0].status,
            TradeStatus::Completed { order_id, price_cents, .. }
            if order_id == "ORDER123" && *price_cents == 15025
        ));
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_empty_database() {
        let pool = setup_test_db().await;

        let result = SchwabExecution::find_pending_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = SchwabExecution::find_by_symbol_and_status(&pool, "", "PENDING")
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

        let result = SchwabExecution::find_pending_by_symbol(&pool, "NONEXISTENT")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = SchwabExecution::find_completed_by_symbol(&pool, "AAPL")
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
                direction: SchwabInstruction::Buy,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "MSFT".to_string(),
                shares: 50,
                direction: SchwabInstruction::Sell,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 200,
                direction: SchwabInstruction::Buy,
                status: TradeStatus::Completed {
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
        let pending_all = SchwabExecution::find_by_symbol_and_status(&pool, "", "PENDING")
            .await
            .unwrap();
        assert_eq!(pending_all.len(), 2); // AAPL and MSFT pending

        let completed_all = SchwabExecution::find_by_symbol_and_status(&pool, "", "COMPLETED")
            .await
            .unwrap();
        assert_eq!(completed_all.len(), 1); // Only AAPL completed

        let failed_all = SchwabExecution::find_by_symbol_and_status(&pool, "", "FAILED")
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
                direction: SchwabInstruction::Buy,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "TSLA".to_string(),
                shares: 200,
                direction: SchwabInstruction::Sell,
                status: TradeStatus::Pending,
            },
            SchwabExecution {
                id: None,
                symbol: "MSFT".to_string(),
                shares: 300,
                direction: SchwabInstruction::Buy,
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
        let aapl_result = SchwabExecution::find_pending_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(aapl_result.len(), 1);
        assert_eq!(aapl_result[0].shares, 100);

        let tsla_result = SchwabExecution::find_pending_by_symbol(&pool, "TSLA")
            .await
            .unwrap();
        assert_eq!(tsla_result.len(), 1);
        assert_eq!(tsla_result[0].shares, 200);

        let msft_result = SchwabExecution::find_pending_by_symbol(&pool, "MSFT")
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
            direction: SchwabInstruction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let result = SchwabExecution::find_pending_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(result.len(), 1);

        let result = SchwabExecution::find_pending_by_symbol(&pool, "aapl")
            .await
            .unwrap();
        assert_eq!(result.len(), 0);

        let result = SchwabExecution::find_pending_by_symbol(&pool, "Aapl")
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
                direction: SchwabInstruction::Buy,
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
            let result = SchwabExecution::find_pending_by_symbol(&pool, symbol)
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
            direction: SchwabInstruction::Sell,
            status: TradeStatus::Completed {
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

        let result = SchwabExecution::find_completed_by_symbol(&pool, "TEST")
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        let found = &result[0];

        // Verify all fields are correctly preserved and converted
        assert_eq!(found.symbol, "TEST");
        assert_eq!(found.shares, 12345);
        assert_eq!(found.direction, SchwabInstruction::Sell);
        assert!(matches!(
            &found.status,
            TradeStatus::Completed { order_id, price_cents, .. }
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
        let count = SchwabExecution::db_count(&pool).await.unwrap();
        assert_eq!(count, 0); // No invalid data should have been inserted
    }

    #[tokio::test]
    async fn test_update_status_within_transaction() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 15,
            direction: SchwabInstruction::Sell,
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
        SchwabExecution::update_status_within_transaction(
            &mut sql_tx,
            id,
            TradeStatus::Completed {
                executed_at: Utc::now(),
                order_id: "ORDER456".to_string(),
                price_cents: 20050,
            },
        )
        .await
        .unwrap();
        sql_tx.commit().await.unwrap();

        // Verify the update persisted by finding executions with the new status
        let completed_executions = SchwabExecution::find_completed_by_symbol(&pool, "TSLA")
            .await
            .unwrap();

        assert_eq!(completed_executions.len(), 1);
        assert!(matches!(
            &completed_executions[0].status,
            TradeStatus::Completed { order_id, price_cents, .. }
            if order_id == "ORDER456" && *price_cents == 20050
        ));
    }

    #[tokio::test]
    async fn test_db_count() {
        let pool = setup_test_db().await;

        let count = SchwabExecution::db_count(&pool).await.unwrap();
        assert_eq!(count, 0);

        let execution = SchwabExecution {
            id: None,
            symbol: "NVDA".to_string(),
            shares: 5,
            direction: SchwabInstruction::Buy,
            status: TradeStatus::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let count = SchwabExecution::db_count(&pool).await.unwrap();
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
            direction: SchwabInstruction::Buy,
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
            direction: SchwabInstruction::Sell,
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
            direction: SchwabInstruction::Buy,
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
            direction: SchwabInstruction::Sell,
            status: TradeStatus::Completed {
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
        let pending_aapl = SchwabExecution::find_pending_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        let completed_aapl = SchwabExecution::find_completed_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(completed_aapl.len(), 1);
    }
}
