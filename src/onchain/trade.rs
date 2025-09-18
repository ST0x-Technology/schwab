use alloy::primitives::{B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::num::ParseFloatError;

use crate::bindings::IOrderBookV4::{ClearV2, OrderV3, TakeOrderV2};
#[cfg(test)]
use crate::error::PersistenceError;
use crate::error::{OnChainError, TradeValidationError};
use crate::onchain::EvmEnv;
use crate::schwab::Direction;
use crate::symbol::cache::SymbolCache;
#[cfg(test)]
use sqlx::SqlitePool;

/// Union of all trade events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TradeEvent {
    ClearV2(Box<ClearV2>),
    TakeOrderV2(Box<TakeOrderV2>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnchainTrade {
    pub id: Option<i64>,
    pub tx_hash: B256,
    pub log_index: u64,
    pub symbol: String,
    pub amount: f64,
    pub direction: Direction,
    pub price_usdc: f64,
    pub created_at: Option<DateTime<Utc>>,
}

impl OnchainTrade {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        let tx_hash_str = self.tx_hash.to_string();
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = self.log_index as i64;

        let direction_str = self.direction.as_str();
        let result = sqlx::query!(
            r#"
            INSERT INTO onchain_trades (tx_hash, log_index, symbol, amount, direction, price_usdc)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            tx_hash_str,
            log_index_i64,
            self.symbol,
            self.amount,
            direction_str,
            self.price_usdc
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }

    #[cfg(test)]
    pub async fn find_by_tx_hash_and_log_index(
        pool: &SqlitePool,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Self, OnChainError> {
        let tx_hash_str = tx_hash.to_string();
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = log_index as i64;
        let row = sqlx::query!(
            "SELECT id, tx_hash, log_index, symbol, amount, direction, price_usdc, created_at FROM onchain_trades WHERE tx_hash = ?1 AND log_index = ?2",
            tx_hash_str,
            log_index_i64
        )
        .fetch_one(pool)
        .await?;

        let tx_hash = row.tx_hash.parse().map_err(|_| {
            OnChainError::Persistence(PersistenceError::InvalidDirection(format!(
                "Invalid tx_hash format: {}",
                row.tx_hash
            )))
        })?;

        let direction = row.direction.parse().map_err(|_| {
            OnChainError::Persistence(PersistenceError::InvalidDirection(format!(
                "Invalid direction in database: {}",
                row.direction
            )))
        })?;

        Ok(Self {
            id: Some(row.id),
            tx_hash,
            #[allow(clippy::cast_sign_loss)]
            log_index: row.log_index as u64,
            symbol: row.symbol,
            amount: row.amount,
            direction,
            price_usdc: row.price_usdc,
            created_at: row
                .created_at
                .map(|naive_dt| DateTime::from_naive_utc_and_offset(naive_dt, Utc)),
        })
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM onchain_trades")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }

    /// Core parsing logic for converting blockchain events to trades
    pub async fn try_from_order_and_fill_details<P: Provider>(
        cache: &SymbolCache,
        provider: P,
        order: OrderV3,
        fill: OrderFill,
        log: Log,
    ) -> Result<Option<Self>, OnChainError> {
        let tx_hash = log.transaction_hash.ok_or(TradeValidationError::NoTxHash)?;
        let log_index = log.log_index.ok_or(TradeValidationError::NoLogIndex)?;

        let input = order
            .validInputs
            .get(fill.input_index)
            .ok_or(TradeValidationError::NoInputAtIndex(fill.input_index))?;

        let output = order
            .validOutputs
            .get(fill.output_index)
            .ok_or(TradeValidationError::NoOutputAtIndex(fill.output_index))?;

        let onchain_input_amount = u256_to_f64(fill.input_amount, input.decimals)?;
        let onchain_input_symbol = cache.get_io_symbol(&provider, input).await?;

        let onchain_output_amount = u256_to_f64(fill.output_amount, output.decimals)?;
        let onchain_output_symbol = cache.get_io_symbol(provider, output).await?;

        let (ticker, onchain_direction) =
            determine_schwab_trade_details(&onchain_input_symbol, &onchain_output_symbol)?;

        if ticker.is_empty() {
            return Ok(None);
        }

        // Use ticker + "0x" to create consistent tokenized symbol
        let tokenized_symbol = format!("{ticker}0x");

        // Extract tokenized equity amount and USDC amount
        let (equity_amount, usdc_amount) = if onchain_output_symbol.ends_with("0x") {
            // Gave away tokenized stock for USDC (onchain sell)
            (onchain_output_amount, onchain_input_amount)
        } else {
            // Gave away USDC for tokenized stock (onchain buy)
            (onchain_input_amount, onchain_output_amount)
        };

        if equity_amount == 0.0 {
            return Ok(None);
        }

        // Calculate price per share in USDC (always USDC amount / equity amount)
        let price_per_share_usdc = usdc_amount / equity_amount;

        if price_per_share_usdc.is_nan() || price_per_share_usdc <= 0.0 {
            return Ok(None);
        }

        let trade = Self {
            id: None,
            tx_hash,
            log_index,
            symbol: tokenized_symbol,
            amount: equity_amount,
            direction: onchain_direction,
            price_usdc: price_per_share_usdc,
            created_at: None,
        };

        Ok(Some(trade))
    }

    /// Attempts to create an OnchainTrade from a transaction hash by looking up
    /// the transaction receipt and parsing relevant orderbook events.
    pub async fn try_from_tx_hash<P: Provider>(
        tx_hash: B256,
        provider: P,
        cache: &SymbolCache,
        env: &EvmEnv,
    ) -> Result<Option<Self>, OnChainError> {
        let receipt = provider
            .get_transaction_receipt(tx_hash)
            .await?
            .ok_or_else(|| {
                OnChainError::Validation(crate::error::TradeValidationError::TransactionNotFound(
                    tx_hash,
                ))
            })?;

        let trades: Vec<_> = receipt
            .inner
            .logs()
            .iter()
            .filter(|log| {
                (log.topic0() == Some(&ClearV2::SIGNATURE_HASH)
                    || log.topic0() == Some(&TakeOrderV2::SIGNATURE_HASH))
                    && log.address() == env.orderbook
            })
            .collect();

        if trades.len() > 1 {
            tracing::warn!(
                "Found {} potential trades in the tx with hash {tx_hash}, returning first match",
                trades.len()
            );
        }

        for log in trades {
            if let Some(trade) =
                try_convert_log_to_onchain_trade(log, &provider, cache, env).await?
            {
                return Ok(Some(trade));
            }
        }

        Ok(None)
    }
}

/// Determines onchain trade direction and ticker based on onchain symbol configuration.
///
/// If the on-chain order has USDC as input and an 0x tokenized stock as
/// output then it means the order received USDC and gave away an 0x
/// tokenized stock, i.e. sold the tokenized stock onchain.
fn determine_schwab_trade_details(
    onchain_input_symbol: &str,
    onchain_output_symbol: &str,
) -> Result<(String, Direction), OnChainError> {
    // USDC input + 0x tokenized stock output = sold tokenized stock onchain
    if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("0x") {
        let ticker = extract_ticker_from_0x_symbol(
            onchain_output_symbol,
            onchain_input_symbol,
            onchain_output_symbol,
        )?;
        return Ok((ticker, Direction::Sell));
    }

    // 0x tokenized stock input + USDC output = bought tokenized stock onchain
    if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("0x") {
        let ticker = extract_ticker_from_0x_symbol(
            onchain_input_symbol,
            onchain_input_symbol,
            onchain_output_symbol,
        )?;
        return Ok((ticker, Direction::Buy));
    }

    Err(TradeValidationError::InvalidSymbolConfiguration(
        onchain_input_symbol.to_string(),
        onchain_output_symbol.to_string(),
    )
    .into())
}

fn extract_ticker_from_0x_symbol(
    tokenized_symbol: &str,
    input_symbol: &str,
    output_symbol: &str,
) -> Result<String, TradeValidationError> {
    tokenized_symbol
        .strip_suffix("0x")
        .map(ToString::to_string)
        .ok_or_else(|| {
            TradeValidationError::InvalidSymbolConfiguration(
                input_symbol.to_string(),
                output_symbol.to_string(),
            )
        })
}

#[derive(Debug)]
pub struct OrderFill {
    pub input_index: usize,
    pub input_amount: U256,
    pub output_index: usize,
    pub output_amount: U256,
}

async fn try_convert_log_to_onchain_trade<P: Provider>(
    log: &Log,
    provider: P,
    cache: &SymbolCache,
    env: &EvmEnv,
) -> Result<Option<OnchainTrade>, OnChainError> {
    let log_with_metadata = Log {
        inner: log.inner.clone(),
        block_hash: log.block_hash,
        block_number: log.block_number,
        block_timestamp: log.block_timestamp,
        transaction_hash: log.transaction_hash,
        transaction_index: log.transaction_index,
        log_index: log.log_index,
        removed: false,
    };

    if let Ok(clear_event) = log.log_decode::<ClearV2>() {
        return OnchainTrade::try_from_clear_v2(
            env,
            cache,
            &provider,
            clear_event.data().clone(),
            log_with_metadata,
        )
        .await;
    }

    if let Ok(take_order_event) = log.log_decode::<TakeOrderV2>() {
        return OnchainTrade::try_from_take_order_if_target_owner(
            cache,
            &provider,
            take_order_event.data().clone(),
            log_with_metadata,
            env.order_owner,
        )
        .await;
    }

    Ok(None)
}

/// Helper that converts a fixed-decimal U256 amount into an f64 using the provided number of decimals.
fn u256_to_f64(amount: U256, decimals: u8) -> Result<f64, ParseFloatError> {
    if amount.is_zero() {
        return Ok(0.);
    }

    let u256_str = amount.to_string();
    let decimals = decimals as usize;

    let formatted = if decimals == 0 {
        u256_str
    } else if u256_str.len() <= decimals {
        format!("0.{}{}", "0".repeat(decimals - u256_str.len()), u256_str)
    } else {
        let (int_part, frac_part) = u256_str.split_at(u256_str.len() - decimals);
        format!("{int_part}.{frac_part}")
    };

    formatted.parse::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::setup_test_db;
    use alloy::primitives::fixed_bytes;

    #[tokio::test]
    async fn test_onchain_trade_save_within_transaction_and_find() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 42,
            symbol: "AAPL0x".to_string(),
            amount: 10.0,
            direction: Direction::Sell,
            price_usdc: 150.25,
            created_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let id = trade.save_within_transaction(&mut sql_tx).await.unwrap();
        sql_tx.commit().await.unwrap();
        assert!(id > 0);

        let found =
            OnchainTrade::find_by_tx_hash_and_log_index(&pool, trade.tx_hash, trade.log_index)
                .await
                .unwrap();

        assert_eq!(found.tx_hash, trade.tx_hash);
        assert_eq!(found.log_index, trade.log_index);
        assert_eq!(found.symbol, trade.symbol);
        assert!((found.amount - trade.amount).abs() < f64::EPSILON);
        assert_eq!(found.direction, trade.direction);
        assert!((found.price_usdc - trade.price_usdc).abs() < f64::EPSILON);
        assert!(found.id.is_some());
        assert!(found.created_at.is_some());
    }

    #[test]
    fn test_extract_ticker_from_0x_symbol_valid() {
        assert_eq!(
            extract_ticker_from_0x_symbol("AAPL0x", "USDC", "AAPL0x").unwrap(),
            "AAPL"
        );
        assert_eq!(
            extract_ticker_from_0x_symbol("TSLA0x", "USDC", "TSLA0x").unwrap(),
            "TSLA"
        );
        assert_eq!(
            extract_ticker_from_0x_symbol("GOOG0x", "USDC", "GOOG0x").unwrap(),
            "GOOG"
        );
    }

    #[test]
    fn test_extract_ticker_from_0x_symbol_invalid() {
        let result = extract_ticker_from_0x_symbol("AAPL", "USDC", "AAPL");
        assert!(matches!(
            result.unwrap_err(),
            TradeValidationError::InvalidSymbolConfiguration(_, _)
        ));

        let result = extract_ticker_from_0x_symbol("", "USDC", "");
        assert!(matches!(
            result.unwrap_err(),
            TradeValidationError::InvalidSymbolConfiguration(_, _)
        ));

        assert_eq!(
            extract_ticker_from_0x_symbol("0x", "USDC", "0x").unwrap(),
            ""
        );
    }

    #[test]
    fn test_determine_schwab_trade_details_usdc_to_0x() {
        let result = determine_schwab_trade_details("USDC", "AAPL0x").unwrap();
        assert_eq!(result.0, "AAPL");
        assert_eq!(result.1, Direction::Sell); // Onchain sold AAPL0x for USDC

        let result = determine_schwab_trade_details("USDC", "TSLA0x").unwrap();
        assert_eq!(result.0, "TSLA");
        assert_eq!(result.1, Direction::Sell); // Onchain sold TSLA0x for USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_0x_to_usdc() {
        let result = determine_schwab_trade_details("AAPL0x", "USDC").unwrap();
        assert_eq!(result.0, "AAPL");
        assert_eq!(result.1, Direction::Buy); // Onchain bought AAPL0x with USDC

        let result = determine_schwab_trade_details("TSLA0x", "USDC").unwrap();
        assert_eq!(result.0, "TSLA");
        assert_eq!(result.1, Direction::Buy); // Onchain bought TSLA0x with USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_invalid_configurations() {
        let result = determine_schwab_trade_details("BTC", "ETH");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("USDC", "USDC");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("AAPL0x", "TSLA0x");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("", "");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));
    }

    #[test]
    fn test_u256_to_f64_edge_cases() {
        assert!((u256_to_f64(U256::ZERO, 18).unwrap() - 0.0).abs() < f64::EPSILON);

        let max_safe = U256::from(9_007_199_254_740_991_u64);
        let result = u256_to_f64(max_safe, 0).unwrap();
        assert!((result - 9_007_199_254_740_991.0).abs() < 1.0);

        let very_large = U256::MAX;
        let result = u256_to_f64(very_large, 18);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_find_by_tx_hash_and_log_index_invalid_formats() {
        let pool = setup_test_db().await;

        // Attempt to insert invalid tx_hash format - should fail due to constraint
        let insert_result = sqlx::query!(
            "INSERT INTO onchain_trades (tx_hash, log_index, symbol, amount, direction, price_usdc) 
             VALUES ('invalid_hash', 1, 'TEST', 1.0, 'BUY', 1.0)"
        )
        .execute(&pool)
        .await;

        // The insert should fail due to tx_hash constraint
        assert!(insert_result.is_err());
    }

    #[tokio::test]
    async fn test_find_by_tx_hash_and_log_index_invalid_direction() {
        let pool = setup_test_db().await;

        // Attempt to insert invalid direction data - should fail due to constraint
        let insert_result = sqlx::query!(
            "INSERT INTO onchain_trades (tx_hash, log_index, symbol, amount, direction, price_usdc) 
             VALUES ('0x1234567890123456789012345678901234567890123456789012345678901234', 1, 'TEST', 1.0, 'INVALID', 1.0)"
        )
        .execute(&pool)
        .await;

        // The insert should fail due to direction constraint
        assert!(insert_result.is_err());
    }

    #[tokio::test]
    async fn test_save_within_transaction_constraint_violation() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 100,
            symbol: "AAPL0x".to_string(),
            amount: 10.0,
            direction: Direction::Buy,
            price_usdc: 150.0,
            created_at: None,
        };

        // Insert first trade
        let mut sql_tx1 = pool.begin().await.unwrap();
        trade.save_within_transaction(&mut sql_tx1).await.unwrap();
        sql_tx1.commit().await.unwrap();

        // Try to insert duplicate trade (same tx_hash and log_index)
        let mut sql_tx2 = pool.begin().await.unwrap();
        let duplicate_result = trade.save_within_transaction(&mut sql_tx2).await;
        assert!(
            duplicate_result.is_err(),
            "Expected duplicate constraint violation"
        );
        sql_tx2.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn test_save_within_transaction_large_log_index_wrapping() {
        let pool = setup_test_db().await;

        // Test that extremely large log_index values are handled consistently
        // u64::MAX will wrap to -1 when cast to i64
        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0x2222222222222222222222222222222222222222222222222222222222222222"
            ),
            log_index: u64::MAX, // Will become -1 when cast to i64
            symbol: "AAPL0x".to_string(),
            amount: 10.0,
            direction: Direction::Buy,
            price_usdc: 150.0,
            created_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();

        // This should fail due to log_index constraint (log_index >= 0)
        let save_result = trade.save_within_transaction(&mut sql_tx).await;
        assert!(save_result.is_err());
        sql_tx.rollback().await.unwrap();
    }

    #[test]
    fn test_u256_to_f64_precision_loss() {
        // Test precision loss with very large numbers
        let very_large = U256::MAX;
        let result = u256_to_f64(very_large, 0).unwrap();
        assert!(result.is_finite());

        // Test with maximum decimals
        let small_amount = U256::from(1);
        let result = u256_to_f64(small_amount, 255).unwrap(); // Max u8 value
        assert!((result - 0.0).abs() < f64::EPSILON); // Should be rounded to 0 due to extreme precision
    }

    #[test]
    fn test_u256_to_f64_formatting_edge_cases() {
        // Test with exactly decimal places length
        let amount = U256::from(123_456);
        let result = u256_to_f64(amount, 6).unwrap();
        assert!((result - 0.123_456).abs() < f64::EPSILON);

        // Test with more decimals than digits
        let amount = U256::from(5);
        let result = u256_to_f64(amount, 10).unwrap();
        assert!((result - 0.000_000_000_5).abs() < f64::EPSILON);

        // Test with zero decimals
        let amount = U256::from(12345);
        let result = u256_to_f64(amount, 0).unwrap();
        assert!((result - 12_345.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_determine_schwab_trade_details_edge_cases() {
        // Test with minimal valid 0x symbol
        let result = determine_schwab_trade_details("USDC", "A0x").unwrap();
        assert_eq!(result.0, "A");
        assert_eq!(result.1, Direction::Sell); // Onchain sold A0x for USDC

        // Test symbol case sensitivity
        let result = determine_schwab_trade_details("usdc", "AAPL0x");
        assert!(
            result.is_err(),
            "Expected case-sensitive USDC matching to fail"
        );

        // Test symbol with 0x but not as suffix
        // Expected 0x prefix to be rejected
        determine_schwab_trade_details("USDC", "0xAAPL").unwrap_err();

        // Test symbol with multiple 0x occurrences - should extract from suffix only
        let (ticker, _) = determine_schwab_trade_details("USDC", "0xAAPL0x").unwrap();
        assert_eq!(ticker, "0xAAPL");
    }

    #[tokio::test]
    async fn test_try_from_tx_hash_transaction_not_found() {
        use crate::onchain::EvmEnv;
        use crate::symbol::cache::SymbolCache;
        use alloy::providers::{ProviderBuilder, mock::Asserter};

        let asserter = Asserter::new();
        // Mock the eth_getTransactionReceipt call to return null (transaction not found)
        asserter.push_success(&serde_json::Value::Null);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();
        let env = EvmEnv {
            ws_rpc_url: "ws://localhost:8545".parse().unwrap(),
            orderbook: alloy::primitives::Address::ZERO,
            order_owner: alloy::primitives::Address::ZERO,
            deployment_block: 0,
        };

        let tx_hash =
            fixed_bytes!("0x4444444444444444444444444444444444444444444444444444444444444444");

        // Mock returns empty response by default, simulating transaction not found
        let result = OnchainTrade::try_from_tx_hash(tx_hash, provider, &cache, &env).await;

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::TransactionNotFound(_))
        ));
    }

    #[test]
    fn test_extract_ticker_from_0x_symbol_empty_ticker() {
        // Test with just "0x" - should return empty string
        let result = extract_ticker_from_0x_symbol("0x", "USDC", "0x").unwrap();
        assert_eq!(result, "");

        // Test with complex ticker extraction
        let result = extract_ticker_from_0x_symbol(
            "VERY.LONG.TICKER.NAME0x",
            "USDC",
            "VERY.LONG.TICKER.NAME0x",
        )
        .unwrap();
        assert_eq!(result, "VERY.LONG.TICKER.NAME");
    }

    #[tokio::test]
    async fn test_find_by_tx_hash_database_error() {
        let pool = setup_test_db().await;

        // Close the pool to simulate database connection error
        pool.close().await;

        // Expected database connection error
        OnchainTrade::find_by_tx_hash_and_log_index(
            &pool,
            fixed_bytes!("0x5555555555555555555555555555555555555555555555555555555555555555"),
            1,
        )
        .await
        .unwrap_err();
    }

    #[tokio::test]
    async fn test_db_count_with_data() {
        let pool = setup_test_db().await;

        // Insert test data
        for i in 0..5 {
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0..31].copy_from_slice(&[
                0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78,
                0x90, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56, 0x78, 0x90, 0x12, 0x34, 0x56,
                0x78, 0x90, 0x12,
            ]);
            tx_hash_bytes[31] = i;

            let trade = OnchainTrade {
                id: None,
                tx_hash: alloy::primitives::B256::from(tx_hash_bytes),
                log_index: u64::from(i),
                symbol: format!("TEST{i}0x"),
                amount: 10.0,
                direction: Direction::Buy,
                price_usdc: 150.0,
                created_at: None,
            };

            let mut sql_tx = pool.begin().await.unwrap();
            trade.save_within_transaction(&mut sql_tx).await.unwrap();
            sql_tx.commit().await.unwrap();
        }

        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 5);
    }
}
