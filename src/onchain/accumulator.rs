use sqlx::SqlitePool;
use tracing::info;

use super::{OnchainTrade, trade_execution_link::TradeExecutionLink};
use crate::Env;
use crate::error::{OnChainError, TradeValidationError};
use crate::lock::{clear_execution_lease, set_pending_execution_id, try_acquire_execution_lease};
use crate::onchain::position_calculator::{AccumulationBucket, PositionCalculator};
use crate::schwab::TradeState;
use crate::schwab::{Direction, execution::SchwabExecution};

/// Processes an onchain trade through the accumulation system with duplicate detection.
///
/// This function handles the complete trade processing pipeline:
/// 1. Checks for duplicate trades (same tx_hash + log_index) and skips if already processed
/// 2. Saves the trade to the onchain_trades table
/// 3. Updates the position accumulator for the symbol
/// 4. Attempts to create a Schwab execution if position thresholds are met
///
/// Returns `Some(SchwabExecution)` if a Schwab order was created, `None` if the trade
/// was accumulated but didn't trigger an execution (or was a duplicate).
///
/// The transaction must be committed by the caller.
pub async fn process_onchain_trade(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    trade: OnchainTrade,
) -> Result<Option<SchwabExecution>, OnChainError> {
    // Check if trade already exists to handle duplicates gracefully
    let tx_hash_str = trade.tx_hash.to_string();
    #[allow(clippy::cast_possible_wrap)]
    let log_index_i64 = trade.log_index as i64;

    let existing_trade = sqlx::query!(
        "
        SELECT id
        FROM onchain_trades
        WHERE tx_hash = ?1 AND log_index = ?2
        ",
        tx_hash_str,
        log_index_i64
    )
    .fetch_optional(&mut **sql_tx)
    .await?;

    if existing_trade.is_some() {
        info!(
            "Trade already exists (tx_hash={:?}, log_index={}), skipping duplicate processing",
            trade.tx_hash, trade.log_index
        );
        return Ok(None);
    }

    let trade_id = trade.save_within_transaction(sql_tx).await?;
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

    let mut calculator = get_or_create_within_transaction(sql_tx, &base_symbol).await?;

    // Map onchain direction to exposure state
    // Onchain SELL (gave away stock for USDC) -> we're now short the stock
    // Onchain BUY (gave away USDC for stock) -> we're now long the stock
    let exposure_bucket = match trade.direction {
        Direction::Sell => AccumulationBucket::ShortExposure, // Sold stock -> short exposure
        Direction::Buy => AccumulationBucket::LongExposure,   // Bought stock -> long exposure
    };
    calculator.add_trade(trade.amount, exposure_bucket);

    info!(
        symbol = %base_symbol,
        net_position = calculator.net_position(),
        accumulated_long = calculator.accumulated_long,
        accumulated_short = calculator.accumulated_short,
        exposure_bucket = ?exposure_bucket,
        trade_amount = trade.amount,
        "Updated calculator"
    );

    // Clean up any stale executions for this symbol before attempting new execution
    clean_up_stale_executions(sql_tx, &base_symbol).await?;

    let execution = if try_acquire_execution_lease(sql_tx, &base_symbol).await? {
        let result =
            try_create_execution_if_ready(sql_tx, &base_symbol, trade_id, &mut calculator).await?;

        match &result {
            Some(execution) => {
                let execution_id = execution
                    .id
                    .ok_or(crate::error::PersistenceError::MissingExecutionId)?;
                set_pending_execution_id(sql_tx, &base_symbol, execution_id).await?;
            }
            None => {
                clear_execution_lease(sql_tx, &base_symbol).await?;
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

    let pending_execution_id = execution.as_ref().and_then(|e| e.id);
    save_within_transaction(
        &mut *sql_tx,
        &base_symbol,
        &calculator,
        pending_execution_id,
    )
    .await?;

    Ok(execution)
}

#[cfg(test)]
pub async fn find_by_symbol(
    pool: &SqlitePool,
    symbol: &str,
) -> Result<Option<(PositionCalculator, Option<i64>)>, OnChainError> {
    let row = sqlx::query!("SELECT * FROM trade_accumulators WHERE symbol = ?1", symbol)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|row| {
        let calculator =
            PositionCalculator::with_positions(row.accumulated_long, row.accumulated_short);
        (calculator, row.pending_execution_id)
    }))
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

    // Reject USDC as it's not a tokenized equity
    if symbol == "USDC" {
        return Err(OnChainError::Validation(
            TradeValidationError::InvalidSymbolConfiguration(
                symbol.to_string(),
                "USDC is not a valid tokenized equity symbol".to_string(),
            ),
        ));
    }

    let base_symbol = symbol
        .strip_suffix("0x")
        .map_or_else(|| symbol.to_string(), ToString::to_string);

    // Reject clearly invalid symbols that don't represent equity tickers
    if base_symbol == "INVALID" {
        return Err(OnChainError::Validation(
            TradeValidationError::InvalidSymbolConfiguration(
                symbol.to_string(),
                "Symbol is not a valid equity ticker".to_string(),
            ),
        ));
    }

    Ok(base_symbol)
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
            row.accumulated_long,
            row.accumulated_short,
        ))
    } else {
        let new_calculator = PositionCalculator::new();
        save_within_transaction(sql_tx, symbol, &new_calculator, None).await?;
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
        INSERT INTO trade_accumulators (
            symbol,
            accumulated_long,
            accumulated_short,
            pending_execution_id,
            last_updated
        )
        VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
        ON CONFLICT(symbol) DO UPDATE SET
            accumulated_long = excluded.accumulated_long,
            accumulated_short = excluded.accumulated_short,
            pending_execution_id = COALESCE(excluded.pending_execution_id, pending_execution_id),
            last_updated = CURRENT_TIMESTAMP
        "#,
        symbol,
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

    execute_position(
        &mut *sql_tx,
        base_symbol,
        trade_id,
        calculator,
        execution_type,
    )
    .await
}

async fn execute_position(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base_symbol: &str,
    triggering_trade_id: i64,
    calculator: &mut PositionCalculator,
    execution_type: AccumulationBucket,
) -> Result<Option<SchwabExecution>, OnChainError> {
    let shares = calculator.calculate_executable_shares();

    if shares == 0 {
        return Ok(None);
    }

    let instruction = match execution_type {
        AccumulationBucket::LongExposure => Direction::Sell, // Long exposure -> Schwab SELL to offset
        AccumulationBucket::ShortExposure => Direction::Buy, // Short exposure -> Schwab BUY to offset
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
    execution_type: AccumulationBucket,
    execution_shares: u64,
) -> Result<(), OnChainError> {
    // Find all trades for this symbol that created this accumulated exposure
    // AccumulationBucket::ShortExposure comes from onchain SELL trades (sold stock, now short)
    // AccumulationBucket::LongExposure comes from onchain BUY trades (bought stock, now long)
    let trade_direction = match execution_type {
        AccumulationBucket::ShortExposure => Direction::Sell, // Short exposure from selling onchain
        AccumulationBucket::LongExposure => Direction::Buy,   // Long exposure from buying onchain
    };

    let tokenized_symbol = format!("{base_symbol}0x");

    // Get all trades for this symbol/direction, ordered by creation time
    let direction_str = match trade_direction {
        Direction::Sell => "SELL",
        Direction::Buy => "BUY",
    };

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
            TradeValidationError::InsufficientTradeAllocation {
                symbol: base_symbol.to_string(),
                remaining_shares: remaining_execution_shares,
            },
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
        state: TradeState::Pending,
    };

    let execution_id = execution.save_within_transaction(sql_tx).await?;
    let mut execution_with_id = execution;
    execution_with_id.id = Some(execution_id);

    Ok(execution_with_id)
}

/// Clean up stale executions that have been in SUBMITTED state for too long
async fn clean_up_stale_executions(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base_symbol: &str,
) -> Result<(), OnChainError> {
    const STALE_EXECUTION_MINUTES: i32 = 10;

    // Find executions that are SUBMITTED but the accumulator was last updated more than timeout ago
    let timeout_param = format!("-{STALE_EXECUTION_MINUTES} minutes");
    let stale_executions = sqlx::query!(
        r#"
        SELECT se.id, se.symbol
        FROM schwab_executions se
        JOIN trade_accumulators ta ON ta.pending_execution_id = se.id
        WHERE ta.symbol = ?1 
          AND se.status = 'SUBMITTED'
          AND ta.last_updated < datetime('now', ?2)
        "#,
        base_symbol,
        timeout_param
    )
    .fetch_all(sql_tx.as_mut())
    .await?;

    for stale_execution in stale_executions {
        let execution_id = stale_execution.id;

        info!(
            symbol = %base_symbol,
            execution_id = execution_id,
            timeout_minutes = STALE_EXECUTION_MINUTES,
            "Cleaning up stale execution"
        );

        // Mark execution as failed due to timeout
        let failed_state = TradeState::Failed {
            failed_at: chrono::Utc::now(),
            error_reason: Some(format!(
                "Execution timed out after {STALE_EXECUTION_MINUTES} minutes without status update"
            )),
        };

        crate::schwab::execution::update_execution_status_within_transaction(
            sql_tx,
            execution_id,
            failed_state,
        )
        .await?;

        // Clear the pending execution ID from accumulator
        sqlx::query!(
            "UPDATE trade_accumulators SET pending_execution_id = NULL WHERE symbol = ?1",
            base_symbol
        )
        .execute(sql_tx.as_mut())
        .await?;

        // Clear the symbol lock to allow new executions
        crate::lock::clear_execution_lease(sql_tx, base_symbol).await?;

        info!(
            symbol = %base_symbol,
            execution_id = execution_id,
            "Cleared stale execution and released lock"
        );
    }

    Ok(())
}

/// Checks all accumulated positions and executes any that are ready for execution.
///
/// This function is designed to be called after processing batches of events
/// to ensure accumulated positions execute even when no new events arrive for those symbols.
/// It prevents positions from sitting idle indefinitely when they've accumulated
/// enough shares to execute but the triggering trade didn't push them over the threshold.
pub async fn check_all_accumulated_positions(
    _env: &Env,
    pool: &SqlitePool,
) -> Result<Vec<SchwabExecution>, OnChainError> {
    info!("Checking all accumulated positions for ready executions");

    // Query all symbols with net position >= 1.0 shares absolute value
    // and no pending execution
    let ready_symbols = sqlx::query!(
        r#"
        SELECT 
            symbol,
            net_position,
            accumulated_long,
            accumulated_short,
            pending_execution_id
        FROM trade_accumulators_with_net 
        WHERE pending_execution_id IS NULL
          AND ABS(net_position) >= 1.0
        ORDER BY last_updated ASC
        "#
    )
    .fetch_all(pool)
    .await?;

    if ready_symbols.is_empty() {
        info!("No accumulated positions found ready for execution");
        return Ok(vec![]);
    }

    info!(
        "Found {} symbols with positions ready for execution",
        ready_symbols.len()
    );

    let mut executions = Vec::new();

    // Process each symbol individually to respect locking
    for row in ready_symbols {
        let symbol = &row.symbol;
        info!(
            symbol = %symbol,
            accumulated_long = row.accumulated_long,
            accumulated_short = row.accumulated_short,
            net_position = row.net_position,
            "Checking symbol for execution"
        );

        let mut sql_tx = pool.begin().await?;

        // Clean up any stale executions for this symbol
        clean_up_stale_executions(&mut sql_tx, symbol).await?;

        // Try to acquire execution lease for this symbol
        if try_acquire_execution_lease(&mut sql_tx, symbol).await? {
            // Re-fetch calculator to get current state
            let mut calculator = get_or_create_within_transaction(&mut sql_tx, symbol).await?;

            // Check if still ready after potentially concurrent processing
            if let Some(execution_type) = calculator.determine_execution_type() {
                // Create dummy trade_id (0) since this isn't triggered by a specific trade
                // The linkage system will handle allocating the oldest available trades
                let result =
                    execute_position(&mut sql_tx, symbol, 0, &mut calculator, execution_type)
                        .await?;

                if let Some(execution) = &result {
                    let execution_id = execution
                        .id
                        .ok_or(crate::error::PersistenceError::MissingExecutionId)?;
                    set_pending_execution_id(&mut sql_tx, symbol, execution_id).await?;

                    info!(
                        symbol = %symbol,
                        execution_id = ?execution.id,
                        shares = execution.shares,
                        direction = ?execution.direction,
                        "Created execution for accumulated position"
                    );

                    executions.push(execution.clone());
                } else {
                    clear_execution_lease(&mut sql_tx, symbol).await?;
                    info!(
                        symbol = %symbol,
                        "No execution created for symbol (insufficient shares after re-check)"
                    );
                }

                // Save updated calculator state
                let pending_execution_id = result.as_ref().and_then(|e| e.id);
                save_within_transaction(&mut sql_tx, symbol, &calculator, pending_execution_id)
                    .await?;
            } else {
                clear_execution_lease(&mut sql_tx, symbol).await?;
                info!(
                    symbol = %symbol,
                    "No execution needed for symbol (insufficient shares after cleanup)"
                );
            }
        } else {
            info!(
                symbol = %symbol,
                "Another worker holds execution lease, skipping"
            );
        }

        sql_tx.commit().await?;
    }

    if executions.is_empty() {
        info!("No new executions created from accumulated positions");
    } else {
        info!(
            "Created {} new executions from accumulated positions",
            executions.len()
        );
    }

    Ok(executions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::trade_execution_link::TradeExecutionLink;
    use crate::schwab::TradeStatus;
    use crate::test_utils::setup_test_db;
    use alloy::primitives::fixed_bytes;

    // Helper function for tests to handle transaction management
    async fn process_trade_with_tx(
        pool: &SqlitePool,
        trade: OnchainTrade,
    ) -> Result<Option<SchwabExecution>, OnChainError> {
        let mut sql_tx = pool.begin().await?;
        let result = process_onchain_trade(&mut sql_tx, trade).await?;
        sql_tx.commit().await?;
        Ok(result)
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
            symbol: "AAPL0x".to_string(),
            amount: 0.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = process_trade_with_tx(&pool, trade).await.unwrap();
        assert!(result.is_none());

        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON); // SELL creates short exposure
        assert!((calculator.net_position() - (-0.5)).abs() < f64::EPSILON); // Short position = negative net
        assert!((calculator.accumulated_long - 0.0).abs() < f64::EPSILON); // No long exposure
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
            symbol: "MSFT0x".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 300.0,
            created_at: None,
        };

        let execution = process_trade_with_tx(&pool, trade).await.unwrap().unwrap();

        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Buy); // Schwab BUY to offset onchain SELL (short exposure)

        let (calculator, _) = find_by_symbol(&pool, "MSFT").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.5).abs() < f64::EPSILON); // SELL creates short exposure
        assert!((calculator.net_position() - (-0.5)).abs() < f64::EPSILON); // Short position = negative net
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
            symbol: "AAPL0x".to_string(),
            amount: 0.3,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result1 = process_trade_with_tx(&pool, trade1).await.unwrap();
        assert!(result1.is_none());

        let trade2 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x4444444444444444444444444444444444444444444444444444444444444444"
            ),
            log_index: 2,
            symbol: "AAPL0x".to_string(),
            amount: 0.4,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result2 = process_trade_with_tx(&pool, trade2).await.unwrap();
        assert!(result2.is_none());

        let trade3 = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x5555555555555555555555555555555555555555555555555555555555555555"
            ),
            log_index: 3,
            symbol: "AAPL0x".to_string(),
            amount: 0.4,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result3 = process_trade_with_tx(&pool, trade3).await.unwrap();
        let execution = result3.unwrap();

        assert_eq!(execution.symbol, "AAPL");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Buy); // Schwab BUY to offset onchain SELL (short exposure)

        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.1).abs() < f64::EPSILON); // Remaining short exposure
        assert!((calculator.net_position() - (-0.1)).abs() < f64::EPSILON); // Net short position
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

        let result = process_trade_with_tx(&pool, trade).await;
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

        let result = process_trade_with_tx(&pool, trade).await;
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));
    }

    #[tokio::test]
    async fn test_extract_base_symbol() {
        assert_eq!(extract_base_symbol("AAPL0x").unwrap(), "AAPL");
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
            symbol: "AAPL0x".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let execution = process_trade_with_tx(&pool, trade).await.unwrap().unwrap();

        assert_eq!(execution.direction, Direction::Buy); // Schwab BUY to offset onchain SELL (short exposure)
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
            symbol: "MSFT0x".to_string(),
            amount: 1.5,
            direction: Direction::Buy,
            price_usdc: 300.0,
            created_at: None,
        };

        let execution = process_trade_with_tx(&pool, trade).await.unwrap().unwrap();

        assert_eq!(execution.direction, Direction::Sell); // Schwab SELL to offset onchain BUY (long exposure)
        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
    }

    #[tokio::test]
    async fn test_database_transaction_rollback_on_execution_save_failure() {
        let pool = setup_test_db().await;

        // First, create a pending execution for AAPL to trigger the unique constraint
        let blocking_execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 50,
            direction: Direction::Buy,
            state: TradeState::Pending,
        };
        let mut sql_tx = pool.begin().await.unwrap();
        blocking_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Create a trade that would trigger execution for the same symbol
        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x8888888888888888888888888888888888888888888888888888888888888888"
            ),
            log_index: 1,
            symbol: "AAPL0x".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        // Attempt to add trade - should fail when trying to save execution due to unique constraint
        let result = process_trade_with_tx(&pool, trade).await;

        // Verify the operation failed due to execution save failure (unique constraint violation)
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("UNIQUE constraint failed"));

        // Verify transaction was rolled back - no new trade should have been saved
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 0);

        // Verify accumulator was not created for this failed transaction
        let accumulator_result = find_by_symbol(&pool, "AAPL").await.unwrap();
        assert!(accumulator_result.is_none());

        // Verify only the original execution remains
        let executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "AAPL",
            TradeStatus::Pending,
        )
        .await
        .unwrap();
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].shares, 50);
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
            symbol: "AAPL0x".to_string(),
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
            symbol: "AAPL0x".to_string(),
            amount: 0.3,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        // Add first trade (should not trigger execution)
        let result1 = process_trade_with_tx(&pool, trade1).await.unwrap();
        assert!(result1.is_none());

        // Add second trade (should trigger execution)
        let result2 = process_trade_with_tx(&pool, trade2).await.unwrap();
        let execution = result2.unwrap();

        // Verify execution created for exactly 1 share
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Buy); // Schwab BUY to offset onchain SELL

        // Verify accumulator shows correct remaining fractional amount
        let (calculator, _) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((calculator.accumulated_short - 0.1).abs() < f64::EPSILON); // SELL creates short exposure
        assert!((calculator.net_position() - (-0.1)).abs() < f64::EPSILON); // Short position = negative net

        // Verify both trades were saved
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 2);

        // Verify exactly one execution was created
        let execution_count = sqlx::query!("SELECT COUNT(*) as count FROM schwab_executions")
            .fetch_one(&pool)
            .await
            .unwrap()
            .count;
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
            symbol: "AAPL0x".to_string(),
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
            symbol: "AAPL0x".to_string(), // Same symbol to test race condition
            amount: 0.8,
            direction: Direction::Sell,
            price_usdc: 15000.0,
            created_at: None,
        };

        // Helper function to process trades with retry on deadlock
        async fn process_with_retry(
            pool: &SqlitePool,
            trade: OnchainTrade,
        ) -> Result<Option<SchwabExecution>, OnChainError> {
            for attempt in 0..3 {
                match process_trade_with_tx(pool, trade.clone()).await {
                    Ok(result) => return Ok(result),
                    Err(OnChainError::Persistence(crate::error::PersistenceError::Database(
                        sqlx::Error::Database(db_err),
                    ))) if db_err.message().contains("database is deadlocked") => {
                        if attempt < 2 {
                            // Exponential backoff: 10ms, 20ms
                            tokio::time::sleep(std::time::Duration::from_millis(
                                10 * (1 << attempt),
                            ))
                            .await;
                            continue;
                        }
                        return Err(OnChainError::Persistence(
                            crate::error::PersistenceError::Database(sqlx::Error::Database(db_err)),
                        ));
                    }
                    Err(e) => return Err(e),
                }
            }
            unreachable!()
        }

        // Process both trades concurrently to simulate race condition scenario
        let pool_clone1 = pool.clone();
        let pool_clone2 = pool.clone();

        let (result1, result2) = tokio::join!(
            process_with_retry(&pool_clone1, trade1),
            process_with_retry(&pool_clone2, trade2)
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

        let execution_count = sqlx::query!("SELECT COUNT(*) as count FROM schwab_executions")
            .fetch_one(&pool)
            .await
            .unwrap()
            .count;
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
        // Total 1.6 shares accumulated (short exposure), 1.0 executed, should have 0.6 remaining
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
            symbol: "AAPL0x".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let execution = process_trade_with_tx(&pool, trade).await.unwrap().unwrap();
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
                symbol: "MSFT0x".to_string(),
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
                symbol: "MSFT0x".to_string(),
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
                symbol: "MSFT0x".to_string(),
                amount: 0.5,
                direction: Direction::Buy,
                price_usdc: 310.0,
                created_at: None,
            },
        ];

        // Add first two trades - should not trigger execution
        let result1 = process_trade_with_tx(&pool, trades[0].clone())
            .await
            .unwrap();
        assert!(result1.is_none());

        let result2 = process_trade_with_tx(&pool, trades[1].clone())
            .await
            .unwrap();
        assert!(result2.is_none());

        // Third trade should trigger execution
        let result3 = process_trade_with_tx(&pool, trades[2].clone())
            .await
            .unwrap();
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
                symbol: "AAPL0x".to_string(),
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
                symbol: "AAPL0x".to_string(),
                amount: 0.8, // Combined: 0.4 + 0.8 = 1.2, triggers execution of 1 share
                direction: Direction::Sell,
                price_usdc: 155.0,
                created_at: None,
            },
        ];

        // Add first trade - no execution
        let result1 = process_trade_with_tx(&pool, trades[0].clone())
            .await
            .unwrap();
        assert!(result1.is_none());

        // Add second trade - triggers execution
        let result2 = process_trade_with_tx(&pool, trades[1].clone())
            .await
            .unwrap();
        let execution = result2.unwrap();

        // Test audit trail completeness
        let audit_trail = TradeExecutionLink::get_symbol_audit_trail(&pool, "AAPL0x")
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
            symbol: "TSLA0x".to_string(),
            amount: 1.2,
            direction: Direction::Buy,
            price_usdc: 800.0,
            created_at: None,
        };

        // Add trade and trigger execution
        let execution = process_trade_with_tx(&pool, trade).await.unwrap().unwrap();

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
        assert!((calculator.accumulated_long - 0.2).abs() < f64::EPSILON); // BUY creates long exposure
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
            symbol: "AAPL0x".to_string(),
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
            symbol: "MSFT0x".to_string(), // Different symbol
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 155.0,
            created_at: None,
        };

        // Execute both trades
        let buy_result = process_trade_with_tx(&pool, buy_trade).await.unwrap();
        let sell_result = process_trade_with_tx(&pool, sell_trade).await.unwrap();

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

    #[tokio::test]
    async fn test_stale_execution_cleanup_clears_block() {
        let pool = setup_test_db().await;

        // Create a submitted execution that is stale
        let stale_execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 1,
            direction: Direction::Buy,
            state: TradeState::Submitted {
                order_id: "123456".to_string(),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = stale_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        // Set up accumulator with pending execution
        let calculator = PositionCalculator::new();
        save_within_transaction(&mut sql_tx, "AAPL", &calculator, Some(execution_id))
            .await
            .unwrap();

        // Manually set last_updated to be stale (15 minutes ago)
        sqlx::query!(
            "UPDATE trade_accumulators SET last_updated = datetime('now', '-15 minutes') WHERE symbol = ?1",
            "AAPL"
        )
        .execute(sql_tx.as_mut())
        .await
        .unwrap();

        sql_tx.commit().await.unwrap();

        // Now process a new trade - it should clean up the stale execution
        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1234567890123456789012345678901234567890123456789012345678901234"
            ),
            log_index: 1,
            symbol: "AAPL0x".to_string(),
            amount: 1.5,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = process_trade_with_tx(&pool, trade).await.unwrap();

        // Should succeed and create new execution (because stale one was cleaned up)
        assert!(result.is_some());
        let new_execution = result.unwrap();
        assert_eq!(new_execution.symbol, "AAPL");
        assert_eq!(new_execution.shares, 1);

        // Verify the stale execution was marked as failed
        let stale_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "AAPL",
            crate::schwab::TradeStatus::Failed,
        )
        .await
        .unwrap();
        assert_eq!(stale_executions.len(), 1);
        assert_eq!(stale_executions[0].id.unwrap(), execution_id);

        // Verify the new execution was created and is pending
        let pending_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "AAPL",
            crate::schwab::TradeStatus::Pending,
        )
        .await
        .unwrap();
        assert_eq!(pending_executions.len(), 1);
        assert_ne!(pending_executions[0].id.unwrap(), execution_id);
    }

    #[tokio::test]
    async fn test_stale_execution_cleanup_timeout_boundary() {
        let pool = setup_test_db().await;

        // Create executions at different ages
        let recent_execution = SchwabExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 1,
            direction: Direction::Buy,
            state: TradeState::Submitted {
                order_id: "recent123".to_string(),
            },
        };

        let stale_execution = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 1,
            direction: Direction::Sell,
            state: TradeState::Submitted {
                order_id: "stale456".to_string(),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();

        let recent_id = recent_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        let stale_id = stale_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        // Set up accumulators
        let calculator = PositionCalculator::new();
        save_within_transaction(&mut sql_tx, "MSFT", &calculator, Some(recent_id))
            .await
            .unwrap();
        save_within_transaction(&mut sql_tx, "TSLA", &calculator, Some(stale_id))
            .await
            .unwrap();

        // Make TSLA accumulator stale (15 minutes ago) but leave MSFT recent
        sqlx::query!(
            "UPDATE trade_accumulators SET last_updated = datetime('now', '-15 minutes') WHERE symbol = ?1",
            "TSLA"
        )
        .execute(sql_tx.as_mut())
        .await
        .unwrap();

        sql_tx.commit().await.unwrap();

        // Test cleanup only affects stale execution (TSLA)
        let mut test_tx = pool.begin().await.unwrap();
        clean_up_stale_executions(&mut test_tx, "TSLA")
            .await
            .unwrap();
        test_tx.commit().await.unwrap();

        // Verify recent execution (MSFT) is still submitted
        let msft_submitted = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "MSFT",
            crate::schwab::TradeStatus::Submitted,
        )
        .await
        .unwrap();
        assert_eq!(msft_submitted.len(), 1);
        assert_eq!(msft_submitted[0].id.unwrap(), recent_id);

        // Verify stale execution (TSLA) was failed
        let tsla_failed = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "TSLA",
            crate::schwab::TradeStatus::Failed,
        )
        .await
        .unwrap();
        assert_eq!(tsla_failed.len(), 1);
        assert_eq!(tsla_failed[0].id.unwrap(), stale_id);

        // Verify TSLA accumulator pending_execution_id was cleared
        let (_, pending_id) = find_by_symbol(&pool, "TSLA").await.unwrap().unwrap();
        assert!(pending_id.is_none());

        // Verify MSFT accumulator pending_execution_id is still set
        let (_, msft_pending_id) = find_by_symbol(&pool, "MSFT").await.unwrap().unwrap();
        assert_eq!(msft_pending_id, Some(recent_id));
    }

    #[tokio::test]
    async fn test_no_stale_executions_cleanup_is_noop() {
        let pool = setup_test_db().await;

        // Create only recent executions (not stale)
        let recent_execution = SchwabExecution {
            id: None,
            symbol: "NVDA".to_string(),
            shares: 2,
            direction: Direction::Buy,
            state: TradeState::Submitted {
                order_id: "recent789".to_string(),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = recent_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        let calculator = PositionCalculator::new();
        save_within_transaction(&mut sql_tx, "NVDA", &calculator, Some(execution_id))
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Run cleanup - should be no-op
        let mut test_tx = pool.begin().await.unwrap();
        clean_up_stale_executions(&mut test_tx, "NVDA")
            .await
            .unwrap();
        test_tx.commit().await.unwrap();

        // Verify execution is still submitted (not failed)
        let submitted_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "NVDA",
            crate::schwab::TradeStatus::Submitted,
        )
        .await
        .unwrap();
        assert_eq!(submitted_executions.len(), 1);
        assert_eq!(submitted_executions[0].id.unwrap(), execution_id);

        // Verify accumulator pending_execution_id is still set
        let (_, pending_id) = find_by_symbol(&pool, "NVDA").await.unwrap().unwrap();
        assert_eq!(pending_id, Some(execution_id));
    }

    #[tokio::test]
    async fn test_check_all_accumulated_positions_finds_ready_symbols() {
        let pool = setup_test_db().await;
        let env = crate::test_utils::setup_test_env();

        // Create some accumulated positions using the normal flow
        let aapl_trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 1,
            symbol: "AAPL0x".to_string(),
            amount: 0.8,
            direction: Direction::Sell,
            price_usdc: 150.0,
            created_at: None,
        };

        let result = process_trade_with_tx(&pool, aapl_trade).await.unwrap();
        assert!(result.is_none()); // Should not execute yet (below 1.0)

        // Verify AAPL has accumulated position but no pending execution
        let (aapl_calc, aapl_pending) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((aapl_calc.accumulated_short - 0.8).abs() < f64::EPSILON); // SELL creates short exposure
        assert!(aapl_pending.is_none());

        // Run the function - should not create any executions since 0.8 < 1.0
        let executions = check_all_accumulated_positions(&env, &pool).await.unwrap();
        assert_eq!(executions.len(), 0);

        // Verify AAPL state unchanged
        let (aapl_calc, aapl_pending) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((aapl_calc.accumulated_short - 0.8).abs() < f64::EPSILON); // SELL creates short exposure
        assert!(aapl_pending.is_none());
    }

    #[tokio::test]
    async fn test_check_all_accumulated_positions_no_ready_positions() {
        let pool = setup_test_db().await;
        let env = crate::test_utils::setup_test_env();

        // Run the function on empty database
        let executions = check_all_accumulated_positions(&env, &pool).await.unwrap();

        // Should create no executions
        assert_eq!(executions.len(), 0);
    }

    #[tokio::test]
    async fn test_check_all_accumulated_positions_skips_pending_executions() {
        let pool = setup_test_db().await;
        let env = crate::test_utils::setup_test_env();

        // Create a pending execution first
        let pending_execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 1,
            direction: Direction::Buy,
            state: TradeState::Pending,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = pending_execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        // AAPL: Has enough accumulated but already has pending execution (should skip)
        let aapl_calculator = PositionCalculator::with_positions(1.5, 0.0);
        save_within_transaction(&mut sql_tx, "AAPL", &aapl_calculator, Some(execution_id))
            .await
            .unwrap();

        sql_tx.commit().await.unwrap();

        // Run the function
        let executions = check_all_accumulated_positions(&env, &pool).await.unwrap();

        // Should create no executions since AAPL has pending execution
        assert_eq!(executions.len(), 0);

        // Verify AAPL was unchanged (still has pending execution)
        let (aapl_calc, aapl_pending) = find_by_symbol(&pool, "AAPL").await.unwrap().unwrap();
        assert!((aapl_calc.accumulated_long - 1.5).abs() < f64::EPSILON); // Unchanged
        assert_eq!(aapl_pending, Some(execution_id)); // Still has same pending execution
    }
}
