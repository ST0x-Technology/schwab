use sqlx::SqlitePool;

use crate::onchain::{TradeConversionError, TradeStatus};
use crate::schwab::SchwabInstruction;

macro_rules! convert_rows_to_executions {
    ($rows:expr) => {{
        let mut executions = Vec::new();

        for row in $rows {
            let direction = row
                .direction
                .parse()
                .map_err(TradeConversionError::InvalidSchwabInstruction)?;

            let status = row
                .status
                .parse()
                .map_err(TradeConversionError::InvalidTradeStatus)?;

            executions.push(SchwabExecution {
                id: Some(row.id),
                symbol: row.symbol,
                #[allow(clippy::cast_sign_loss)]
                shares: row.shares as u64,
                direction,
                order_id: row.order_id,
                #[allow(clippy::cast_sign_loss)]
                price_cents: row.price_cents.map(|p| p as u64),
                status,
                executed_at: row.executed_at.map(|dt| dt.to_string()),
            });
        }

        Ok(executions)
    }};
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchwabExecution {
    pub id: Option<i64>,
    pub symbol: String,
    pub shares: u64,
    pub direction: SchwabInstruction,
    pub order_id: Option<String>,
    pub price_cents: Option<u64>,
    pub status: TradeStatus,
    pub executed_at: Option<String>,
}

impl SchwabExecution {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        #[allow(clippy::cast_possible_wrap)]
        let shares_i64 = self.shares as i64;
        let direction_str = self.direction.as_str();
        #[allow(clippy::cast_possible_wrap)]
        let price_cents_i64 = self.price_cents.map(|p| p as i64);
        let status_str = self.status.as_str();

        let result = sqlx::query!(
            r#"
            INSERT INTO schwab_executions (symbol, shares, direction, order_id, price_cents, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            self.symbol,
            shares_i64,
            direction_str,
            self.order_id,
            price_cents_i64,
            status_str
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn find_by_symbol_and_status(
        pool: &SqlitePool,
        symbol: &str,
        status: TradeStatus,
    ) -> Result<Vec<Self>, TradeConversionError> {
        let status_str = status.as_str();

        if symbol.is_empty() {
            let rows = sqlx::query!(
                "SELECT * FROM schwab_executions WHERE status = ?1 ORDER BY id ASC",
                status_str
            )
            .fetch_all(pool)
            .await?;

            convert_rows_to_executions!(rows)
        } else {
            let rows = sqlx::query!(
                "SELECT * FROM schwab_executions WHERE symbol = ?1 AND status = ?2 ORDER BY id ASC",
                symbol,
                status_str
            )
            .fetch_all(pool)
            .await?;

            convert_rows_to_executions!(rows)
        }
    }

    pub async fn update_status_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        execution_id: i64,
        new_status: TradeStatus,
        order_id: Option<String>,
        price_cents: Option<u64>,
    ) -> Result<(), sqlx::Error> {
        let status_str = new_status.as_str();
        #[allow(clippy::cast_possible_wrap)]
        let price_cents_i64 = price_cents.map(|p| p as i64);
        sqlx::query!(
            "UPDATE schwab_executions SET status = ?1, order_id = ?2, price_cents = ?3 WHERE id = ?4",
            status_str,
            order_id,
            price_cents_i64,
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
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_schwab_execution_save_and_find() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: SchwabInstruction::Buy,
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
        };

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
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
        };

        let execution2 = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 25,
            direction: SchwabInstruction::Sell,
            order_id: Some("ORDER123".to_string()),
            price_cents: Some(15025),
            status: TradeStatus::Completed,
            executed_at: None,
        };

        let execution3 = SchwabExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 10,
            direction: SchwabInstruction::Buy,
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
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

        let pending_aapl =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Pending)
                .await
                .unwrap();

        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(pending_aapl[0].shares, 50);
        assert_eq!(pending_aapl[0].direction, SchwabInstruction::Buy);

        let completed_aapl =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Completed)
                .await
                .unwrap();

        assert_eq!(completed_aapl.len(), 1);
        assert_eq!(completed_aapl[0].shares, 25);
        assert_eq!(completed_aapl[0].direction, SchwabInstruction::Sell);
        assert_eq!(completed_aapl[0].order_id, Some("ORDER123".to_string()));
        assert_eq!(completed_aapl[0].price_cents, Some(15025));
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_empty_database() {
        let pool = setup_test_db().await;

        // Test with empty database
        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Pending)
                .await
                .unwrap();
        assert_eq!(result.len(), 0);

        // Test with empty symbol (should find all with status)
        let result = SchwabExecution::find_by_symbol_and_status(&pool, "", TradeStatus::Pending)
            .await
            .unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_nonexistent_matches() {
        let pool = setup_test_db().await;

        // Add some test data
        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: SchwabInstruction::Buy,
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Test non-existent symbol
        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "NONEXISTENT", TradeStatus::Pending)
                .await
                .unwrap();
        assert_eq!(result.len(), 0);

        // Test existing symbol with non-existent status combination
        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Completed)
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
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
            },
            SchwabExecution {
                id: None,
                symbol: "MSFT".to_string(),
                shares: 50,
                direction: SchwabInstruction::Sell,
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
            },
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 200,
                direction: SchwabInstruction::Buy,
                order_id: Some("ORDER123".to_string()),
                price_cents: Some(15000),
                status: TradeStatus::Completed,
                executed_at: None,
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
        let pending_all =
            SchwabExecution::find_by_symbol_and_status(&pool, "", TradeStatus::Pending)
                .await
                .unwrap();
        assert_eq!(pending_all.len(), 2); // AAPL and MSFT pending

        let completed_all =
            SchwabExecution::find_by_symbol_and_status(&pool, "", TradeStatus::Completed)
                .await
                .unwrap();
        assert_eq!(completed_all.len(), 1); // Only AAPL completed

        let failed_all = SchwabExecution::find_by_symbol_and_status(&pool, "", TradeStatus::Failed)
            .await
            .unwrap();
        assert_eq!(failed_all.len(), 0); // None failed
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_ordering() {
        let pool = setup_test_db().await;

        // Add executions in a single transaction to test ordering by id
        let executions = vec![
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 100,
                direction: SchwabInstruction::Buy,
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
            },
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 200,
                direction: SchwabInstruction::Sell,
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
            },
            SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 300,
                direction: SchwabInstruction::Buy,
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
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

        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Pending)
                .await
                .unwrap();

        // Should be ordered by id ASC, so first saved should be first
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].shares, 100); // First saved
        assert_eq!(result[1].shares, 200); // Second saved  
        assert_eq!(result[2].shares, 300); // Third saved
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status_case_sensitivity() {
        let pool = setup_test_db().await;

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(), // Uppercase
            shares: 100,
            direction: SchwabInstruction::Buy,
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Test exact match
        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "AAPL", TradeStatus::Pending)
                .await
                .unwrap();
        assert_eq!(result.len(), 1);

        // Test case mismatch (should not find anything - symbols are case-sensitive)
        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "aapl", TradeStatus::Pending)
                .await
                .unwrap();
        assert_eq!(result.len(), 0);

        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "Aapl", TradeStatus::Pending)
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
                order_id: None,
                price_cents: None,
                status: TradeStatus::Pending,
                executed_at: None,
            };

            execution
                .save_within_transaction(&mut sql_tx)
                .await
                .unwrap();
        }
        sql_tx.commit().await.unwrap();

        // Test each symbol can be found
        for symbol in ["BRK.B", "BF-B", "TEST123", "A-B_C.D"] {
            let result =
                SchwabExecution::find_by_symbol_and_status(&pool, symbol, TradeStatus::Pending)
                    .await
                    .unwrap();
            assert_eq!(result.len(), 1, "Failed to find symbol: {}", symbol);
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
            order_id: Some("ORDER789".to_string()),
            price_cents: Some(98765),
            status: TradeStatus::Completed,
            executed_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let result =
            SchwabExecution::find_by_symbol_and_status(&pool, "TEST", TradeStatus::Completed)
                .await
                .unwrap();

        assert_eq!(result.len(), 1);
        let found = &result[0];

        // Verify all fields are correctly preserved and converted
        assert_eq!(found.symbol, "TEST");
        assert_eq!(found.shares, 12345);
        assert_eq!(found.direction, SchwabInstruction::Sell);
        assert_eq!(found.order_id, Some("ORDER789".to_string()));
        assert_eq!(found.price_cents, Some(98765));
        assert_eq!(found.status, TradeStatus::Completed);
        assert!(found.id.is_some());
        assert!(found.executed_at.is_none());
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
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
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
            TradeStatus::Completed,
            Some("ORDER456".to_string()),
            Some(20050),
        )
        .await
        .unwrap();
        sql_tx.commit().await.unwrap();

        // Verify the update persisted by finding executions with the new status
        let completed_executions =
            SchwabExecution::find_by_symbol_and_status(&pool, "TSLA", TradeStatus::Completed)
                .await
                .unwrap();

        assert_eq!(completed_executions.len(), 1);
        assert_eq!(
            completed_executions[0].order_id,
            Some("ORDER456".to_string())
        );
        assert_eq!(completed_executions[0].price_cents, Some(20050));
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
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
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
}
