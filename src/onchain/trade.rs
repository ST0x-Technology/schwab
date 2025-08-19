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
            OnChainError::Persistence(PersistenceError::InvalidSchwabInstruction(format!(
                "Invalid tx_hash format: {}",
                row.tx_hash
            )))
        })?;

        let direction = row.direction.parse().map_err(|_| {
            OnChainError::Persistence(PersistenceError::InvalidSchwabInstruction(format!(
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

        let (schwab_ticker, schwab_direction) =
            Self::determine_schwab_trade_details(&onchain_input_symbol, &onchain_output_symbol)?;

        if schwab_ticker.is_empty() {
            return Ok(None);
        }

        // Use schwab_ticker + "s1" to create consistent tokenized symbol
        let tokenized_symbol = format!("{schwab_ticker}s1");

        // Calculate trade amount based on direction
        let trade_amount = if schwab_direction == Direction::Buy {
            onchain_output_amount.max(0.0)
        } else {
            onchain_input_amount.max(0.0)
        };

        if trade_amount == 0.0 {
            return Ok(None);
        }

        // Calculate price per share in USDC
        let price_per_share_usdc = if schwab_direction == Direction::Buy {
            onchain_input_amount / onchain_output_amount
        } else {
            onchain_output_amount / onchain_input_amount
        };

        if price_per_share_usdc.is_nan() || price_per_share_usdc <= 0.0 {
            return Ok(None);
        }

        let trade = Self {
            id: None,
            tx_hash,
            log_index,
            symbol: tokenized_symbol,
            amount: trade_amount,
            direction: schwab_direction,
            price_usdc: price_per_share_usdc,
            created_at: None,
        };

        Ok(Some(trade))
    }

    /// Determines Schwab trade direction and ticker based on onchain symbol configuration.
    fn determine_schwab_trade_details(
        onchain_input_symbol: &str,
        onchain_output_symbol: &str,
    ) -> Result<(String, Direction), OnChainError> {
        // USDC input + s1 tokenized stock output = sold tokenized stock = buy on Schwab
        if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
            let ticker = Self::extract_ticker_from_s1_symbol(
                onchain_output_symbol,
                onchain_input_symbol,
                onchain_output_symbol,
            )?;
            return Ok((ticker, Direction::Buy));
        }

        // s1 tokenized stock input + USDC output = bought tokenized stock = sell on Schwab
        if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("s1") {
            let ticker = Self::extract_ticker_from_s1_symbol(
                onchain_input_symbol,
                onchain_input_symbol,
                onchain_output_symbol,
            )?;
            return Ok((ticker, Direction::Sell));
        }

        Err(TradeValidationError::InvalidSymbolConfiguration(
            onchain_input_symbol.to_string(),
            onchain_output_symbol.to_string(),
        )
        .into())
    }

    fn extract_ticker_from_s1_symbol(
        s1_symbol: &str,
        input_symbol: &str,
        output_symbol: &str,
    ) -> Result<String, TradeValidationError> {
        s1_symbol
            .strip_suffix("s1")
            .map(ToString::to_string)
            .ok_or_else(|| {
                TradeValidationError::InvalidSymbolConfiguration(
                    input_symbol.to_string(),
                    output_symbol.to_string(),
                )
            })
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
        return OnchainTrade::try_from_take_order_if_target_order(
            cache,
            &provider,
            take_order_event.data().clone(),
            log_with_metadata,
            env.order_hash,
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
            symbol: "AAPLs1".to_string(),
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
}
