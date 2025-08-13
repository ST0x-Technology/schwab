use crate::onchain::{TradeStatus, trade::OnchainTrade};
use crate::schwab::{SchwabInstruction, execution::SchwabExecution};
use sqlx::SqlitePool;

/// Simple data model representing the junction table between executions and trades.
/// Contains no query methods - all querying should be done via properly typed result objects.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionTrade {
    pub schwab_execution_id: i64,
    pub onchain_trade_id: i64,
    pub executed_amount: f64,
}

/// Result type representing a Schwab execution with its associated onchain trades.
/// Constructed from SQL joins rather than separate queries.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionWithTrades {
    pub execution: SchwabExecution,
    pub trades: Vec<OnchainTradeWithAmount>,
}

/// Result type representing an onchain trade with its executed amount.
/// Used when we need trade details plus junction table data.
#[derive(Debug, Clone, PartialEq)]
pub struct OnchainTradeWithAmount {
    pub trade: OnchainTrade,
    pub executed_amount: f64,
}

/// Query functions that return properly typed results from SQL joins.
pub struct ExecutionTradeQueries;

impl ExecutionTradeQueries {
    /// Get a Schwab execution with all its associated onchain trades.
    /// Uses SQL joins to get properly typed results in a single query.
    pub async fn find_execution_with_trades(
        pool: &SqlitePool,
        schwab_execution_id: i64,
    ) -> Result<Option<ExecutionWithTrades>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT 
                e.id as execution_id, e.symbol as execution_symbol, e.shares, e.direction, 
                e.order_id, e.price_cents, e.status as execution_status, e.executed_at,
                t.id as trade_id, t.tx_hash, t.log_index, t.symbol as trade_symbol, 
                t.amount, t.price_usdc, t.created_at,
                et.executed_amount
            FROM schwab_executions e
            LEFT JOIN execution_trades et ON e.id = et.schwab_execution_id
            LEFT JOIN onchain_trades t ON et.onchain_trade_id = t.id
            WHERE e.id = ?1
            ORDER BY t.id
            "#,
            schwab_execution_id
        )
        .fetch_all(pool)
        .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        // First row contains execution data
        let first_row = &rows[0];
        let execution = SchwabExecution {
            id: Some(first_row.execution_id),
            symbol: first_row.execution_symbol.clone(),
            shares: u64::try_from(first_row.shares)
                .map_err(|e| sqlx::Error::Decode(format!("Invalid shares value: {e}").into()))?,
            direction: first_row
                .direction
                .parse::<SchwabInstruction>()
                .map_err(|e| sqlx::Error::Decode(format!("Invalid direction: {e}").into()))?,
            order_id: first_row.order_id.clone(),
            price_cents: first_row.price_cents.map(|p| u64::try_from(p).unwrap_or(0)),
            status: first_row
                .execution_status
                .parse::<TradeStatus>()
                .map_err(|e| sqlx::Error::Decode(format!("Invalid status: {e}").into()))?,
            executed_at: first_row.executed_at.map(|dt| dt.to_string()),
        };

        // Collect all trades (filter out null trades from LEFT JOIN)
        let trades: Vec<OnchainTradeWithAmount> = rows
            .into_iter()
            .filter_map(|row| {
                // Check if we have trade data (some fields may be required vs optional due to LEFT JOIN)
                match (
                    &row.tx_hash,
                    &row.trade_symbol,
                    row.amount,
                    row.price_usdc,
                    row.executed_amount,
                ) {
                    (
                        Some(tx_hash),
                        Some(symbol),
                        Some(amount),
                        Some(price_usdc),
                        Some(executed_amount),
                    ) => {
                        let tx_hash_parsed = tx_hash.parse().ok()?;
                        let trade = OnchainTrade {
                            id: Some(row.trade_id),
                            tx_hash: tx_hash_parsed,
                            log_index: u64::try_from(row.log_index.unwrap_or(0)).unwrap_or(0),
                            symbol: symbol.clone(),
                            amount,
                            price_usdc,
                            created_at: row.created_at.map(|dt| dt.to_string()),
                        };
                        Some(OnchainTradeWithAmount {
                            trade,
                            executed_amount,
                        })
                    }
                    _ => None,
                }
            })
            .collect();

        Ok(Some(ExecutionWithTrades { execution, trades }))
    }

    /// Get all onchain trades for a specific Schwab execution.
    /// Returns just the trades with their executed amounts.
    pub async fn find_trades_for_execution(
        pool: &SqlitePool,
        schwab_execution_id: i64,
    ) -> Result<Vec<OnchainTradeWithAmount>, sqlx::Error> {
        let rows = sqlx::query!(
            r#"
            SELECT 
                t.id, t.tx_hash, t.log_index, t.symbol, t.amount, t.price_usdc, t.created_at,
                et.executed_amount
            FROM onchain_trades t
            JOIN execution_trades et ON t.id = et.onchain_trade_id
            WHERE et.schwab_execution_id = ?1
            ORDER BY t.id
            "#,
            schwab_execution_id
        )
        .fetch_all(pool)
        .await?;

        let trades: Vec<OnchainTradeWithAmount> = rows
            .into_iter()
            .filter_map(|row| {
                let tx_hash_parsed = row.tx_hash.parse().ok()?;
                let trade = OnchainTrade {
                    id: Some(row.id),
                    tx_hash: tx_hash_parsed,
                    log_index: u64::try_from(row.log_index).unwrap_or(0),
                    symbol: row.symbol,
                    amount: row.amount,
                    price_usdc: row.price_usdc,
                    created_at: row.created_at.map(|dt| dt.to_string()),
                };
                Some(OnchainTradeWithAmount {
                    trade,
                    executed_amount: row.executed_amount,
                })
            })
            .collect();

        Ok(trades)
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM execution_trades")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::fixed_bytes;
    use crate::onchain::TradeStatus;
    use crate::schwab::SchwabInstruction;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    async fn create_test_execution(pool: &SqlitePool) -> i64 {
        sqlx::query!(
            r#"
            INSERT INTO schwab_executions (symbol, shares, direction, status)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            "AAPL",
            1,
            "BUY",
            "PENDING"
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    async fn create_test_trade(pool: &SqlitePool, symbol: &str, amount: f64) -> i64 {
        // Get current count to ensure unique tx_hash/log_index combination
        let count_row = sqlx::query!("SELECT COUNT(*) as count FROM onchain_trades")
            .fetch_one(pool)
            .await
            .unwrap();
        let unique_log_index = count_row.count + 1;
        
        let unique_tx_hash = format!(
            "0x{:064x}", 
            0x1111111111111111u64 + u64::try_from(unique_log_index).unwrap_or(1)
        );
        
        sqlx::query!(
            r#"
            INSERT INTO onchain_trades (tx_hash, log_index, symbol, amount, price_usdc)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            unique_tx_hash,
            unique_log_index,
            symbol,
            amount,
            150.0
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    async fn link_execution_to_trade(
        pool: &SqlitePool,
        execution_id: i64,
        trade_id: i64,
        executed_amount: f64,
    ) {
        sqlx::query!(
            r#"
            INSERT INTO execution_trades (schwab_execution_id, onchain_trade_id, executed_amount)
            VALUES (?1, ?2, ?3)
            "#,
            execution_id,
            trade_id,
            executed_amount
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_find_execution_with_trades_not_found() {
        let pool = setup_test_db().await;

        let result = ExecutionTradeQueries::find_execution_with_trades(&pool, 999)
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_execution_with_trades_no_linked_trades() {
        let pool = setup_test_db().await;
        let execution_id = create_test_execution(&pool).await;

        let result = ExecutionTradeQueries::find_execution_with_trades(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.execution.id, Some(execution_id));
        assert_eq!(result.execution.symbol, "AAPL");
        assert_eq!(result.execution.shares, 1);
        assert_eq!(result.execution.direction, SchwabInstruction::Buy);
        assert_eq!(result.execution.status, TradeStatus::Pending);
        assert!(result.trades.is_empty());
    }

    #[tokio::test]
    async fn test_find_execution_with_trades_single_trade() {
        let pool = setup_test_db().await;
        let execution_id = create_test_execution(&pool).await;
        let trade_id = create_test_trade(&pool, "AAPLs1", 1.5).await;
        
        link_execution_to_trade(&pool, execution_id, trade_id, 1.0).await;

        let result = ExecutionTradeQueries::find_execution_with_trades(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.execution.id, Some(execution_id));
        assert_eq!(result.trades.len(), 1);
        
        let trade_with_amount = &result.trades[0];
        assert_eq!(trade_with_amount.trade.id, Some(trade_id));
        assert_eq!(trade_with_amount.trade.symbol, "AAPLs1");
        assert_eq!(trade_with_amount.trade.amount, 1.5);
        assert_eq!(trade_with_amount.executed_amount, 1.0);
        // Verify tx_hash is properly formatted (but will be unique per test)
        assert!(trade_with_amount.trade.tx_hash.to_string().starts_with("0x"));
    }

    #[tokio::test]
    async fn test_find_execution_with_trades_multiple_trades() {
        let pool = setup_test_db().await;
        let execution_id = create_test_execution(&pool).await;
        
        let trade1_id = create_test_trade(&pool, "AAPLs1", 0.7).await;
        let trade2_id = create_test_trade(&pool, "AAPLs1", 0.4).await;
        
        link_execution_to_trade(&pool, execution_id, trade1_id, 0.7).await;
        link_execution_to_trade(&pool, execution_id, trade2_id, 0.3).await;

        let result = ExecutionTradeQueries::find_execution_with_trades(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.execution.id, Some(execution_id));
        assert_eq!(result.trades.len(), 2);

        // Verify trades are ordered by id
        assert!(result.trades[0].trade.id < result.trades[1].trade.id);
        assert_eq!(result.trades[0].executed_amount, 0.7);
        assert_eq!(result.trades[1].executed_amount, 0.3);
    }

    #[tokio::test]
    async fn test_find_trades_for_execution_empty() {
        let pool = setup_test_db().await;

        let result = ExecutionTradeQueries::find_trades_for_execution(&pool, 999)
            .await
            .unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_find_trades_for_execution_multiple_trades() {
        let pool = setup_test_db().await;
        let execution_id = create_test_execution(&pool).await;
        
        let trade1_id = create_test_trade(&pool, "MSFTs1", 0.8).await;
        let trade2_id = create_test_trade(&pool, "MSFTs1", 0.6).await;
        
        link_execution_to_trade(&pool, execution_id, trade1_id, 0.8).await;
        link_execution_to_trade(&pool, execution_id, trade2_id, 0.2).await;

        let result = ExecutionTradeQueries::find_trades_for_execution(&pool, execution_id)
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].trade.symbol, "MSFTs1");
        assert_eq!(result[0].trade.amount, 0.8);
        assert_eq!(result[0].executed_amount, 0.8);
        
        assert_eq!(result[1].trade.symbol, "MSFTs1");
        assert_eq!(result[1].trade.amount, 0.6);
        assert_eq!(result[1].executed_amount, 0.2);
    }

    #[tokio::test]
    async fn test_execution_trade_data_model() {
        let execution_trade = ExecutionTrade {
            schwab_execution_id: 1,
            onchain_trade_id: 2,
            executed_amount: 1.5,
        };

        assert_eq!(execution_trade.schwab_execution_id, 1);
        assert_eq!(execution_trade.onchain_trade_id, 2);
        assert_eq!(execution_trade.executed_amount, 1.5);
    }

    #[tokio::test]
    async fn test_onchain_trade_with_amount_data_model() {
        let trade = OnchainTrade {
            id: Some(1),
            tx_hash: fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            price_usdc: 150.0,
            created_at: Some("2025-01-01T00:00:00Z".to_string()),
        };

        let trade_with_amount = OnchainTradeWithAmount {
            trade: trade.clone(),
            executed_amount: 1.0,
        };

        assert_eq!(trade_with_amount.trade, trade);
        assert_eq!(trade_with_amount.executed_amount, 1.0);
    }

    #[tokio::test]
    async fn test_execution_with_trades_data_model() {
        let execution = SchwabExecution {
            id: Some(1),
            symbol: "AAPL".to_string(),
            shares: 1,
            direction: SchwabInstruction::Buy,
            order_id: None,
            price_cents: None,
            status: TradeStatus::Pending,
            executed_at: None,
        };

        let trade = OnchainTrade {
            id: Some(1),
            tx_hash: fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            price_usdc: 150.0,
            created_at: None,
        };

        let execution_with_trades = ExecutionWithTrades {
            execution: execution.clone(),
            trades: vec![OnchainTradeWithAmount {
                trade,
                executed_amount: 1.0,
            }],
        };

        assert_eq!(execution_with_trades.execution, execution);
        assert_eq!(execution_with_trades.trades.len(), 1);
        assert_eq!(execution_with_trades.trades[0].executed_amount, 1.0);
    }

    #[tokio::test]
    async fn test_db_count() {
        let pool = setup_test_db().await;

        let count = ExecutionTradeQueries::db_count(&pool).await.unwrap();
        assert_eq!(count, 0);

        let execution_id = create_test_execution(&pool).await;
        let trade_id = create_test_trade(&pool, "AAPLs1", 1.0).await;
        link_execution_to_trade(&pool, execution_id, trade_id, 1.0).await;

        let count = ExecutionTradeQueries::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_find_execution_handles_invalid_data_gracefully() {
        let pool = setup_test_db().await;
        
        // Create execution with invalid shares value to test error handling
        sqlx::query!(
            r#"
            INSERT INTO schwab_executions (symbol, shares, direction, status)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            "AAPL",
            -1, // Invalid negative shares
            "BUY",
            "PENDING"
        )
        .execute(&pool)
        .await
        .unwrap();

        let result = ExecutionTradeQueries::find_execution_with_trades(&pool, 1).await;
        assert!(result.is_err());
    }
}
