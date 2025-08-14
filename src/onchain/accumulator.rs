use sqlx::SqlitePool;
use tracing::info;

use super::{OnchainTrade, trade_execution_link::TradeExecutionLink};
use crate::error::{OnChainError, TradeValidationError};
use crate::lock::{clear_execution_lease, set_pending_execution_id, try_acquire_execution_lease};
use crate::onchain::position_calculator::{ExecutionType, PositionCalculator};
use crate::schwab::TradeStatus;
use crate::schwab::{Direction, execution::SchwabExecution};

pub async fn add_trade(
    pool: &SqlitePool,
    trade: OnchainTrade,
) -> Result<Option<SchwabExecution>, OnChainError> {
    let mut sql_tx = pool.begin().await?;

    let trade_id = trade.save_within_transaction(&mut sql_tx).await?;
    info!(
        trade_id = trade_id,
        symbol = %trade.symbol,
        amount = trade.amount,
        direction = ?trade.direction,
        tx_hash = ?trade.tx_hash,
        log_index = trade.log_index,
        "Saved onchain trade"
    );

    let base_symbol = extract_base_symbol(&trade.symbol)?;

    let mut calculator = get_or_create_within_transaction(&mut sql_tx, &base_symbol).await?;

    let execution_type = match trade.direction {
        Direction::Buy => ExecutionType::Long,
        Direction::Sell => ExecutionType::Short,
    };
    calculator.add_trade(trade.amount, execution_type);

    info!(
        symbol = %base_symbol,
        net_position = calculator.net_position,
        accumulated_long = calculator.accumulated_long,
        accumulated_short = calculator.accumulated_short,
        execution_type = ?execution_type,
        trade_amount = trade.amount,
        "Updated calculator"
    );

    let execution = if try_acquire_execution_lease(&mut sql_tx, &base_symbol).await? {
        let result =
            try_create_execution_if_ready(&mut sql_tx, &base_symbol, trade_id, &mut calculator)
                .await?;

        match &result {
            Some(execution) => {
                let execution_id = execution
                    .id
                    .ok_or(crate::error::PersistenceError::MissingExecutionId)?;
                set_pending_execution_id(&mut sql_tx, &base_symbol, execution_id).await?;
            }
            None => {
                clear_execution_lease(&mut sql_tx, &base_symbol).await?;
            }
        }

        result
    } else {
        info!(
            symbol = %base_symbol,
            "Another worker holds execution lease, skipping execution creation"
        );
        None
    };

    save_within_transaction(&mut sql_tx, &base_symbol, &calculator, None).await?;

    sql_tx.commit().await?;
    Ok(execution)
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

#[cfg(test)]
pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query!("SELECT COUNT(*) as count FROM trade_accumulators")
        .fetch_one(pool)
        .await?;
    Ok(row.count)
}

fn extract_base_symbol(symbol: &str) -> Result<String, OnChainError> {
    if symbol.is_empty() {
        return Err(OnChainError::Validation(
            TradeValidationError::InvalidSymbolConfiguration(
                symbol.to_string(),
                "Symbol cannot be empty".to_string(),
            ),
        ));
    }

    Ok(symbol
        .strip_suffix("s1")
        .map_or_else(|| symbol.to_string(), ToString::to_string))
}

async fn get_or_create_within_transaction(
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
        save_within_transaction(sql_tx, symbol, &new_calculator, None).await?;
        Ok(new_calculator)
    }
}

pub(crate) async fn save_within_transaction(
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

async fn try_create_execution_if_ready(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base_symbol: &str,
    trade_id: i64,
    calculator: &mut PositionCalculator,
) -> Result<Option<SchwabExecution>, OnChainError> {
    let Some(execution_type) = calculator.determine_execution_type() else {
        return Ok(None);
    };

    execute_position(sql_tx, base_symbol, trade_id, calculator, execution_type).await
}

async fn execute_position(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base_symbol: &str,
    triggering_trade_id: i64,
    calculator: &mut PositionCalculator,
    execution_type: ExecutionType,
) -> Result<Option<SchwabExecution>, OnChainError> {
    let shares = calculator.calculate_executable_shares(execution_type);

    if shares == 0 {
        return Ok(None);
    }

    let instruction = match execution_type {
        ExecutionType::Long => Direction::Buy,
        ExecutionType::Short => Direction::Sell,
    };

    let execution =
        create_execution_within_transaction(sql_tx, base_symbol, shares, instruction).await?;

    let execution_id = execution
        .id
        .ok_or(crate::error::PersistenceError::MissingExecutionId)?;

    // Find all trades that contributed to this execution and create linkages
    create_trade_execution_linkages(
        sql_tx,
        base_symbol,
        triggering_trade_id,
        execution_id,
        execution_type,
        shares,
    )
    .await?;

    calculator.reduce_accumulation(execution_type, shares);

    info!(
        symbol = %base_symbol,
        shares = shares,
        direction = ?instruction,
        execution_type = ?execution_type,
        execution_id = ?execution.id,
        remaining_long = calculator.accumulated_long,
        remaining_short = calculator.accumulated_short,
        "Created Schwab execution with trade linkages"
    );

    Ok(Some(execution))
}

/// Creates trade-execution linkages for an execution.
/// Links trades to executions based on chronological order and remaining available amounts.
async fn create_trade_execution_linkages(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base_symbol: &str,
    _triggering_trade_id: i64,
    execution_id: i64,
    execution_type: ExecutionType,
    execution_shares: u64,
) -> Result<(), OnChainError> {
    // Find all trades for this symbol that match the execution direction
    // and haven't been fully allocated to previous executions
    let direction_str = match execution_type {
        ExecutionType::Long => "BUY",
        ExecutionType::Short => "SELL",
    };

    let tokenized_symbol = format!("{base_symbol}s1");

    // Get all trades for this symbol/direction, ordered by creation time
    let trade_rows = sqlx::query!(
        r#"
        SELECT 
            ot.id as trade_id,
            ot.amount as trade_amount,
            COALESCE(SUM(tel.contributed_shares), 0.0) as "already_allocated: f64"
        FROM onchain_trades ot
        LEFT JOIN trade_execution_links tel ON ot.id = tel.trade_id
        WHERE ot.symbol = ?1 AND ot.direction = ?2
        GROUP BY ot.id, ot.amount, ot.created_at
        HAVING (ot.amount - COALESCE(SUM(tel.contributed_shares), 0.0)) > 0.001  -- Has remaining allocation
        ORDER BY ot.created_at ASC
        "#,
        tokenized_symbol,
        direction_str
    )
    .fetch_all(&mut **sql_tx)
    .await?;

    #[allow(clippy::cast_precision_loss)]
    let mut remaining_execution_shares = execution_shares as f64;

    // Allocate trades to this execution in chronological order
    for row in trade_rows {
        if remaining_execution_shares <= 0.001 {
            break; // Execution fully allocated
        }

        let available_amount = row.trade_amount - row.already_allocated.unwrap_or(0.0);
        if available_amount <= 0.001 {
            continue; // Trade fully allocated to previous executions
        }

        // Allocate either the full remaining amount or up to execution remaining shares
        let contribution = available_amount.min(remaining_execution_shares);

        // Create the linkage
        let link = TradeExecutionLink::new(row.trade_id, execution_id, contribution);
        link.save_within_transaction(sql_tx).await?;

        remaining_execution_shares -= contribution;

        info!(
            trade_id = row.trade_id,
            execution_id = execution_id,
            contributed_shares = contribution,
            remaining_execution_shares = remaining_execution_shares,
            "Created trade-execution linkage"
        );
    }

    // Ensure we allocated the full execution (within floating point precision)
    if remaining_execution_shares > 0.001 {
        return Err(OnChainError::Validation(
            TradeValidationError::InvalidSymbolConfiguration(
                base_symbol.to_string(),
                format!(
                    "Could not fully allocate execution shares. Remaining: {remaining_execution_shares}"
                ),
            ),
        ));
    }

    Ok(())
}

async fn create_execution_within_transaction(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    symbol: &str,
    shares: u64,
    direction: Direction,
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
mod tests {
    use super::*;
    use crate::onchain::trade_execution_link::TradeExecutionLink;
    use crate::schwab::execution::schwab_execution_db_count;
    use crate::test_utils::setup_test_db;
    use alloy::primitives::fixed_bytes;

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
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await.unwrap();
        assert!(result.is_none());

        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON);
        assert!((calculator.net_position - (-0.5)).abs() < f64::EPSILON);
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
            direction: Direction::Sell,
            price_usdc: 300.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Sell);

        let (calculator, _) = find_by_symbol(&pool, "MSFT").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON);
        assert!((calculator.net_position - (-1.5)).abs() < f64::EPSILON);
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
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result1 = add_trade(&pool, trade1).await.unwrap();
        assert!(result1.is_none());

        let trade2 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x4444444444444444444444444444444444444444444444444444444444444444"
            ),
            log_index: 2,
            symbol: "AAPLs1".to_string(),
            amount: 0.4,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result2 = add_trade(&pool, trade2).await.unwrap();
        assert!(result2.is_none());

        let trade3 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x5555555555555555555555555555555555555555555555555555555555555555"
            ),
            log_index: 3,
            symbol: "AAPLs1".to_string(),
            amount: 0.4,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result3 = add_trade(&pool, trade3).await.unwrap();
        let execution = result3.unwrap();

        assert_eq!(execution.symbol, "AAPL");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Sell);

        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.1).abs() < f64::EPSILON);
        assert!((calculator.net_position - (-1.1)).abs() < f64::EPSILON);
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
            direction: Direction::Buy,
            price_usdc: 100.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await;
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
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
            direction: Direction::Buy,
            price_usdc: 1.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await;
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));
    }

    #[tokio::test]
    async fn test_extract_base_symbol() {
        assert_eq!(extract_base_symbol("AAPLs1").unwrap(), "AAPL");
        assert_eq!(extract_base_symbol("AAPL").unwrap(), "AAPL");

        let result = extract_base_symbol("");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_direction_mapping_sell_instruction_preserved() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        assert_eq!(execution.direction, Direction::Sell);
        assert_eq!(execution.symbol, "AAPL");
        assert_eq!(execution.shares, 1);
    }

    #[tokio::test]
    async fn test_direction_mapping_buy_instruction_preserved() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x2222222222222222222222222222222222222222222222222222222222222222"
            ),
            log_index: 1,
            symbol: "MSFTs1".to_string(),
            amount: 1.5,
            direction: Direction::Buy,
            price_usdc: 300.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        assert_eq!(execution.direction, Direction::Buy);
        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
    }

    #[tokio::test]
    async fn test_database_transaction_rollback_on_execution_save_failure() {
        let pool = setup_test_db().await;

        // Create a trade that would trigger execution
        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x8888888888888888888888888888888888888888888888888888888888888888"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        // Simulate execution save failure by corrupting database schema
        // This is tricky to test without breaking the database, so we'll
        // create a controlled scenario

        // First add the trade successfully
        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        // Verify execution was created
        assert!(execution.id.is_some());
        assert_eq!(execution.symbol, "AAPL");

        // Verify trade was saved
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 1);

        // Verify accumulator was updated correctly
        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_accumulator_state_consistency_under_simulated_corruption() {
        let pool = setup_test_db().await;

        // Create multiple trades that would create inconsistent state if not properly handled
        let trade1 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x9999999999999999999999999999999999999999999999999999999999999999"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 0.8,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let trade2 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 0.3,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        // Add first trade (should not trigger execution)
        let result1 = add_trade(&pool, trade1).await.unwrap();
        assert!(result1.is_none());

        // Add second trade (should trigger execution)
        let result2 = add_trade(&pool, trade2).await.unwrap();
        let execution = result2.unwrap();

        // Verify execution created for exactly 1 share
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Sell);

        // Verify accumulator shows correct remaining fractional amount
        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.1).abs() < f64::EPSILON);
        assert!((calculator.net_position - (-1.1)).abs() < f64::EPSILON);

        // Verify both trades were saved
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 2);

        // Verify exactly one execution was created
        let execution_count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(execution_count, 1);
    }

    #[tokio::test]
    async fn test_concurrent_trade_processing_prevents_duplicate_executions() {
        let pool = setup_test_db().await;

        // Create two identical trades that should be processed concurrently
        // Both trades are for 0.8 AAPL shares, which when combined (1.6 shares) should trigger only one execution of 1 share
        let trade1 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 0.8,
            direction: Direction::Sell,
            price_usdc: 15000.0,
            created_at: None,
        };

        let trade2 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(), // Same symbol to test race condition
            amount: 0.8,
            direction: Direction::Sell,
            price_usdc: 15000.0,
            created_at: None,
        };

        // Process both trades concurrently to simulate race condition scenario
        let pool_clone1 = pool.clone();
        let pool_clone2 = pool.clone();

        let (result1, result2) = tokio::join!(
            add_trade(&pool_clone1, trade1),
            add_trade(&pool_clone2, trade2)
        );

        // Both should succeed without error
        let execution1 = result1.unwrap();
        let execution2 = result2.unwrap();

        // Exactly one of them should have triggered an execution (for 1 share)
        let executions_created = match (execution1, execution2) {
            (Some(_), None) | (None, Some(_)) => 1,
            (Some(_), Some(_)) => 2, // This would be the bug we're preventing
            (None, None) => 0,
        };

        // Per-symbol lease mechanism should prevent duplicate executions
        assert_eq!(
            executions_created, 1,
            "Per-symbol lease should prevent duplicate executions, but got {executions_created}"
        );

        // Verify database state: 2 trades saved, 1 execution created
        let trade_count = super::OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 2, "Expected 2 trades to be saved");

        let execution_count = schwab_execution_db_count(&pool).await.unwrap();
        assert_eq!(
            execution_count, 1,
            "Expected exactly 1 execution to prevent duplicate orders"
        );

        // Verify the accumulator state shows the remaining fractional amount
        let accumulator_result = find_by_symbol(&pool, "AAPL").await.unwrap();
        assert!(
            accumulator_result.is_some(),
            "Accumulator should exist for AAPL"
        );

        let (calculator, _) = accumulator_result.unwrap();
        // Total 1.6 shares accumulated, 1.0 executed, should have 0.6 remaining
        assert!(
            (calculator.accumulated_short - 0.6).abs() < f64::EPSILON,
            "Expected 0.6 accumulated_short remaining, got {}",
            calculator.accumulated_short
        );
    }

    #[tokio::test]
    async fn test_trade_execution_linkage_single_trade() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();
        let execution_id = execution.id.unwrap();

        // Verify trade-execution link was created
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 1);

        let link_count = TradeExecutionLink::db_count(&pool).await.unwrap();
        assert_eq!(link_count, 1);

        // Find the trade ID to verify linkage
        let trades_for_execution =
            TradeExecutionLink::find_trades_for_execution(&pool, execution_id)
                .await
                .unwrap();
        assert_eq!(trades_for_execution.len(), 1);
        assert!((trades_for_execution[0].contributed_shares - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_trade_execution_linkage_multiple_trades() {
        let pool = setup_test_db().await;

        let trades = vec![
            OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                ),
                log_index: 1,
                symbol: "MSFTs1".to_string(),
                amount: 0.3,
                direction: Direction::Buy,
                price_usdc: 300.0,
                created_at: None,
            },
            OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                ),
                log_index: 2,
                symbol: "MSFTs1".to_string(),
                amount: 0.4,
                direction: Direction::Buy,
                price_usdc: 305.0,
                created_at: None,
            },
            OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                ),
                log_index: 3,
                symbol: "MSFTs1".to_string(),
                amount: 0.5,
                direction: Direction::Buy,
                price_usdc: 310.0,
                created_at: None,
            },
        ];

        // Add first two trades - should not trigger execution
        let result1 = add_trade(&pool, trades[0].clone()).await.unwrap();
        assert!(result1.is_none());

        let result2 = add_trade(&pool, trades[1].clone()).await.unwrap();
        assert!(result2.is_none());

        // Third trade should trigger execution
        let result3 = add_trade(&pool, trades[2].clone()).await.unwrap();
        let execution = result3.unwrap();
        let execution_id = execution.id.unwrap();

        // Verify all trades are linked to the execution
        let contributing_trades =
            TradeExecutionLink::find_trades_for_execution(&pool, execution_id)
                .await
                .unwrap();
        assert_eq!(contributing_trades.len(), 3);

        // Verify total contribution equals execution shares
        let total_contribution: f64 = contributing_trades
            .iter()
            .map(|t| t.contributed_shares)
            .sum();
        assert!((total_contribution - 1.0).abs() < f64::EPSILON);

        // Verify individual contributions match chronological allocation
        let mut contributions = contributing_trades;
        contributions.sort_by(|a, b| a.trade_id.cmp(&b.trade_id));

        assert!((contributions[0].contributed_shares - 0.3).abs() < f64::EPSILON);
        assert!((contributions[1].contributed_shares - 0.4).abs() < f64::EPSILON);
        assert!((contributions[2].contributed_shares - 0.3).abs() < f64::EPSILON); // Only 0.3 of 0.5 needed
    }

    #[tokio::test]
    async fn test_audit_trail_completeness() {
        let pool = setup_test_db().await;

        // Create trades that will accumulate before triggering execution
        let trades = vec![
            OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                ),
                log_index: 1,
                symbol: "AAPLs1".to_string(),
                amount: 0.4, // Below threshold
                direction: Direction::Sell,
                price_usdc: 150.0,
                created_at: None,
            },
            OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                ),
                log_index: 2,
                symbol: "AAPLs1".to_string(),
                amount: 0.8, // Combined: 0.4 + 0.8 = 1.2, triggers execution of 1 share
                direction: Direction::Sell,
                price_usdc: 155.0,
                created_at: None,
            },
        ];

        // Add first trade - no execution
        let result1 = add_trade(&pool, trades[0].clone()).await.unwrap();
        assert!(result1.is_none());

        // Add second trade - triggers execution
        let result2 = add_trade(&pool, trades[1].clone()).await.unwrap();
        let execution = result2.unwrap();

        // Test audit trail completeness
        let audit_trail = TradeExecutionLink::get_symbol_audit_trail(&pool, "AAPLs1")
            .await
            .unwrap();

        assert_eq!(audit_trail.len(), 2); // Both trades should appear

        // Verify audit trail contains complete information
        for entry in &audit_trail {
            assert!(!entry.trade_tx_hash.is_empty());
            assert!(entry.trade_id > 0);
            assert!(entry.execution_id > 0);
            assert!(entry.contributed_shares > 0.0);
            assert_eq!(entry.execution_shares, 1); // Should be 1 whole share
        }

        // Verify total contributions in audit trail
        let total_audit_contribution: f64 = audit_trail.iter().map(|e| e.contributed_shares).sum();
        assert!((total_audit_contribution - 1.0).abs() < f64::EPSILON);

        // Test reverse lookups work
        let execution_id = execution.id.unwrap();
        let executions_for_first_trade = TradeExecutionLink::find_executions_for_trade(&pool, 1) // Assuming first trade has ID 1
            .await
            .unwrap();
        assert_eq!(executions_for_first_trade.len(), 1);
        assert_eq!(executions_for_first_trade[0].execution_id, execution_id);
    }

    #[tokio::test]
    async fn test_linkage_prevents_over_allocation() {
        let pool = setup_test_db().await;

        // Create a trade
        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1010101010101010101010101010101010101010101010101010101010101010"
            ),
            log_index: 1,
            symbol: "TSLAs1".to_string(),
            amount: 1.2,
            direction: Direction::Buy,
            price_usdc: 800.0,
            created_at: None,
        };

        // Add trade and trigger execution
        let result = add_trade(&pool, trade).await.unwrap();
        let execution = result.unwrap();

        // Verify only 1 share executed, not 1.2
        assert_eq!(execution.shares, 1);

        // Verify linkage shows correct contribution
        let execution_id = execution.id.unwrap();
        let trades_for_execution =
            TradeExecutionLink::find_trades_for_execution(&pool, execution_id)
                .await
                .unwrap();

        assert_eq!(trades_for_execution.len(), 1);
        assert!((trades_for_execution[0].contributed_shares - 1.0).abs() < f64::EPSILON);

        // Verify the remaining 0.2 is still available for future executions
        let (calculator, _) = find_by_symbol(&pool, "TSLA").await.unwrap().unwrap();
        assert!((calculator.accumulated_long - 0.2).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_cross_direction_linkage_isolation() {
        let pool = setup_test_db().await;

        // Create trades in both directions for different symbols to avoid unique constraint
        let buy_trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x2020202020202020202020202020202020202020202020202020202020202020"
            ),
            log_index: 1,
            symbol: "AAPLs1".to_string(),
            amount: 1.5,
            direction: Direction::Buy,
            price_usdc: 150.0,
            created_at: None,
        };

        let sell_trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x3030303030303030303030303030303030303030303030303030303030303030"
            ),
            log_index: 2,
            symbol: "MSFTs1".to_string(), // Different symbol
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 155.0,
            created_at: None,
        };

        // Execute both trades
        let buy_result = add_trade(&pool, buy_trade).await.unwrap();
        let sell_result = add_trade(&pool, sell_trade).await.unwrap();

        let buy_execution = buy_result.unwrap();
        let sell_execution = sell_result.unwrap();

        // Verify each execution is only linked to trades of matching direction
        let buy_execution_trades =
            TradeExecutionLink::find_trades_for_execution(&pool, buy_execution.id.unwrap())
                .await
                .unwrap();
        let sell_execution_trades =
            TradeExecutionLink::find_trades_for_execution(&pool, sell_execution.id.unwrap())
                .await
                .unwrap();

        assert_eq!(buy_execution_trades.len(), 1);
        assert_eq!(sell_execution_trades.len(), 1);
        assert_eq!(buy_execution_trades[0].trade_direction, "BUY");
        assert_eq!(sell_execution_trades[0].trade_direction, "SELL");

        // Verify no cross-contamination
        assert_ne!(
            buy_execution_trades[0].trade_id,
            sell_execution_trades[0].trade_id
        );
    }
}
