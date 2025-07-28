use alloy::hex;
use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::transports::{RpcError, TransportErrorKind};
use clap::Parser;
use sqlx::SqlitePool;
use std::num::ParseFloatError;

use crate::bindings::IOrderBookV4::OrderV3;
use crate::symbol_cache::SymbolCache;

mod clear;
mod take_order;

#[derive(Parser, Debug, Clone)]
pub struct EvmEnv {
    #[clap(short, long, env)]
    pub ws_rpc_url: url::Url,
    #[clap(short = 'b', long, env)]
    pub orderbook: Address,
    #[clap(short, long, env)]
    pub order_hash: B256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchwabInstruction {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeStatus {
    Pending,
    Completed,
    Failed,
}

impl serde::Serialize for SchwabInstruction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        })
    }
}

impl TradeStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }
}

impl std::str::FromStr for TradeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PENDING" => Ok(Self::Pending),
            "COMPLETED" => Ok(Self::Completed),
            "FAILED" => Ok(Self::Failed),
            _ => Err(format!("Invalid trade status: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub tx_hash: B256,
    pub log_index: u64,

    pub onchain_input_symbol: String,
    pub onchain_input_amount: f64,
    pub onchain_output_symbol: String,
    pub onchain_output_amount: f64,
    pub onchain_io_ratio: f64,
    pub onchain_price_per_share_cents: u64,

    pub schwab_ticker: String,
    pub schwab_instruction: SchwabInstruction,
    pub schwab_quantity: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum TradeConversionError {
    #[error("No transaction hash found in log")]
    NoTxHash,
    #[error("No log index found in log")]
    NoLogIndex,
    #[error("No block number found in log")]
    NoBlockNumber,
    #[error("Invalid IO index: {0}")]
    InvalidIndex(#[from] FromUintError<usize>),
    #[error("No AfterClear log found for ClearV2 log")]
    NoAfterClearLog,
    #[error("No input found at index: {0}")]
    NoInputAtIndex(usize),
    #[error("No output found at index: {0}")]
    NoOutputAtIndex(usize),
    #[error("Failed to get symbol: {0}")]
    GetSymbol(#[from] alloy::contract::Error),
    #[error("Failed to acquire symbol map lock")]
    SymbolMapLock,
    #[error("Expected IO to contain USDC and one s1-suffixed symbol but got {0} and {1}")]
    InvalidSymbolConfiguration(String, String),
    #[error("Failed to convert U256 to f64: {0}")]
    U256ToF64(#[from] ParseFloatError),
    #[error("Sol type error: {0}")]
    SolType(#[from] alloy::sol_types::Error),
    #[error("RPC transport error: {0}")]
    RpcTransport(#[from] RpcError<TransportErrorKind>),
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

struct OrderFill {
    input_index: usize,
    input_amount: U256,
    output_index: usize,
    output_amount: U256,
}

impl Trade {
    async fn try_from_order_and_fill_details<P: Provider>(
        cache: &SymbolCache,
        provider: P,
        order: OrderV3,
        fill: OrderFill,
        log: Log,
    ) -> Result<Option<Self>, TradeConversionError> {
        let tx_hash = log.transaction_hash.ok_or(TradeConversionError::NoTxHash)?;
        let log_index = log.log_index.ok_or(TradeConversionError::NoLogIndex)?;

        let input = order
            .validInputs
            .get(fill.input_index)
            .ok_or(TradeConversionError::NoInputAtIndex(fill.input_index))?;

        let output = order
            .validOutputs
            .get(fill.output_index)
            .ok_or(TradeConversionError::NoOutputAtIndex(fill.output_index))?;

        let onchain_input_amount = u256_to_f64(fill.input_amount, input.decimals)?;
        let onchain_input_symbol = cache.get_io_symbol(&provider, input).await?;

        let onchain_output_amount = u256_to_f64(fill.output_amount, output.decimals)?;
        let onchain_output_symbol = cache.get_io_symbol(provider, output).await?;

        // If the on-chain order has USDC as input and an s1 tokenized stock as
        // output then it means the order received USDC and gave away an s1
        // tokenized stock, i.e. sold, which means that to take the opposite
        // trade in schwab we need to buy and vice versa.
        let (schwab_ticker, schwab_instruction) =
            if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
                let ticker = onchain_output_symbol
                    .strip_suffix("s1")
                    .map(std::string::ToString::to_string)
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Buy)
            } else if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("s1") {
                let ticker = onchain_input_symbol
                    .strip_suffix("s1")
                    .map(std::string::ToString::to_string)
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Sell)
            } else {
                return Err(TradeConversionError::InvalidSymbolConfiguration(
                    onchain_input_symbol,
                    onchain_output_symbol,
                ));
            };

        let onchain_io_ratio = onchain_input_amount / onchain_output_amount;

        // if we're buying on schwab then we sold onchain, so we need to divide the onchain output amount
        // by the input amount. if we're selling on schwab then we bought onchain, so we need to divide the
        // onchain input amount by the output amount.
        let onchain_price_per_share_usdc = if schwab_instruction == SchwabInstruction::Buy {
            onchain_input_amount / onchain_output_amount
        } else {
            onchain_output_amount / onchain_input_amount
        };

        if onchain_price_per_share_usdc.is_nan() {
            return Ok(None);
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let onchain_price_per_share_cents = (onchain_price_per_share_usdc * 100.0) as u64;

        let schwab_quantity = if schwab_instruction == SchwabInstruction::Buy {
            onchain_output_amount
        } else {
            onchain_input_amount
        };

        let trade = Self {
            tx_hash,
            log_index,

            onchain_input_symbol,
            onchain_input_amount,
            onchain_output_symbol,
            onchain_output_amount,
            onchain_io_ratio,
            onchain_price_per_share_cents,

            schwab_ticker,
            schwab_instruction,
            schwab_quantity,
        };

        Ok(Some(trade))
    }

    pub async fn save_to_db(&self, pool: &SqlitePool) -> Result<(), TradeConversionError> {
        let tx_hash_hex = hex::encode_prefixed(self.tx_hash.as_slice());
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = self.log_index as i64;
        let schwab_instruction_str = match self.schwab_instruction {
            SchwabInstruction::Buy => "BUY",
            SchwabInstruction::Sell => "SELL",
        };
        #[allow(clippy::cast_possible_wrap)]
        let price_cents_i64 = self.onchain_price_per_share_cents as i64;
        let status_str = TradeStatus::Pending.as_str();

        sqlx::query!(
            r#"
            INSERT INTO trades (
                tx_hash, log_index,
                onchain_input_symbol, onchain_input_amount,
                onchain_output_symbol, onchain_output_amount, onchain_io_ratio,
                schwab_ticker, schwab_instruction, schwab_quantity, schwab_price_cents,
                status, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
            "#,
            tx_hash_hex,
            log_index_i64,
            self.onchain_input_symbol,
            self.onchain_input_amount,
            self.onchain_output_symbol,
            self.onchain_output_amount,
            self.onchain_io_ratio,
            self.schwab_ticker,
            schwab_instruction_str,
            self.schwab_quantity,
            price_cents_i64,
            status_str,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn update_status(
        pool: &SqlitePool,
        tx_hash: B256,
        log_index: u64,
        status: TradeStatus,
    ) -> Result<(), TradeConversionError> {
        let tx_hash_hex = hex::encode_prefixed(tx_hash.as_slice());
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = log_index as i64;
        let status_str = status.as_str();

        sqlx::query!(
            "UPDATE trades SET status = ?, completed_at = datetime('now') WHERE tx_hash = ? AND log_index = ?",
            status_str,
            tx_hash_hex,
            log_index_i64
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    pub async fn exists_in_db(
        pool: &SqlitePool,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<bool, TradeConversionError> {
        let tx_hash_hex = hex::encode_prefixed(tx_hash.as_slice());
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = log_index as i64;

        let result = sqlx::query!(
            "SELECT COUNT(*) as count FROM trades WHERE tx_hash = ? AND log_index = ?",
            tx_hash_hex,
            log_index_i64
        )
        .fetch_one(pool)
        .await?;

        Ok(result.count > 0)
    }
}

/// Helper that converts a fixed‐decimal `U256` amount into an `f64` using
/// the provided number of decimals.
///
/// NOTE: Parsing should never fail but precision may be lost.
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
    use alloy::primitives::{U256, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::SolCall;
    use std::str::FromStr;

    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::test_utils::{get_test_log, get_test_order};

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_ok_buy_schwab() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        let expected_trade = Trade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 100.0,
            onchain_output_symbol: "FOOs1".to_string(),
            onchain_output_amount: 9.0,
            onchain_io_ratio: 100.0 / 9.0,
            onchain_price_per_share_cents: 1111,
            schwab_ticker: "FOO".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 9.0,
        };

        assert_eq!(trade, expected_trade);

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            get_test_order(),
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.onchain_input_symbol, "USDC");
        assert_eq!(trade.onchain_output_symbol, "FOOs1");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert_eq!(trade.schwab_ticker, "FOO");
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_ok_sell_schwab() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 1,
                input_amount: U256::from_str("9000000000000000000").unwrap(),
                output_index: 0,
                output_amount: U256::from(100_000_000),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        let expected_trade = Trade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 9.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 100.0,
            onchain_io_ratio: 9.0 / 100.0,
            onchain_price_per_share_cents: 1111,
            schwab_ticker: "BAR".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
            schwab_quantity: 9.0,
        };

        assert_eq!(trade, expected_trade);
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_missing_tx_hash() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let mut log = get_test_log();
        log.transaction_hash = None;
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            log,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradeConversionError::NoTxHash));
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_missing_log_index() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let mut log = get_test_log();
        log.log_index = None;
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            log,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradeConversionError::NoLogIndex));
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_invalid_input_index() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 99,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradeConversionError::NoInputAtIndex(99)));
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_invalid_output_index() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 99,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradeConversionError::NoOutputAtIndex(99)));
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_symbol_fetch_failure() {
        let asserter = Asserter::new();
        asserter.push_failure_msg("symbol fetch failed");

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, TradeConversionError::GetSymbol(_)));
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_invalid_symbol_configuration_usdc_usdc() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "USDC" && output == "USDC")
        );
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_invalid_symbol_configuration_no_s1_suffix() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOO".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "USDC" && output == "FOO")
        );
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_err_invalid_symbol_configuration_both_s1() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let err = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            TradeConversionError::InvalidSymbolConfiguration(ref input, ref output)
            if input == "FOOs1" && output == "BARs1"
        ));
    }

    #[test]
    fn test_u256_to_f64() {
        assert!((u256_to_f64(U256::ZERO, 6).unwrap() - 0.0).abs() < f64::EPSILON);

        let amount = U256::from_str("1_000_000_000_000_000_000").unwrap(); // 1.0
        assert!((u256_to_f64(amount, 18).unwrap() - 1.0).abs() < f64::EPSILON);

        let amount = U256::from(123_456_789u64);
        let expected = 123.456_789_f64;
        assert!((u256_to_f64(amount, 6).unwrap() - expected).abs() < f64::EPSILON);

        let amount = U256::from(123u64);
        let expected = 0.000_123_f64;
        assert!((u256_to_f64(amount, 6).unwrap() - expected).abs() < f64::EPSILON);

        let amount = U256::from(999u64);
        assert!((u256_to_f64(amount, 0).unwrap() - 999.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_schwab_instruction_serialize() {
        let buy_json = serde_json::to_string(&SchwabInstruction::Buy).unwrap();
        assert_eq!(buy_json, "\"BUY\"");

        let sell_json = serde_json::to_string(&SchwabInstruction::Sell).unwrap();
        assert_eq!(sell_json, "\"SELL\"");
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_zero_input_amount() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let result = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 1,
                input_amount: U256::ZERO,
                output_index: 0,
                output_amount: U256::from(100_000_000),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert!((result.onchain_input_amount - 0.0).abs() < f64::EPSILON);
        assert_eq!(result.schwab_instruction, SchwabInstruction::Sell);
        assert_eq!(result.onchain_price_per_share_cents, u64::MAX);
        assert!((result.schwab_quantity - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_input_s1_suffix_empty_ticker() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"s1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 1,
                input_amount: U256::from_str("9000000000000000000").unwrap(),
                output_index: 0,
                output_amount: U256::from(100_000_000),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.schwab_ticker, "");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Sell);
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_output_s1_suffix_empty_ticker() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"s1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100_000_000),
                output_index: 1,
                output_amount: U256::from_str("9000000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.schwab_ticker, "");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_save_to_db_success() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let trade = Trade {
            tx_hash: fixed_bytes!(
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd"
            ),
            log_index: 123,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            onchain_price_per_share_cents: 20000,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5.0,
        };

        trade.save_to_db(&pool).await.unwrap();

        let saved_trade = sqlx::query!(
            "SELECT * FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(saved_trade.onchain_input_symbol.unwrap(), "USDC");
        assert!((saved_trade.onchain_input_amount.unwrap() - 1000.0).abs() < f64::EPSILON);
        assert_eq!(saved_trade.onchain_output_symbol.unwrap(), "AAPLs1");
        assert!((saved_trade.onchain_output_amount.unwrap() - 5.0).abs() < f64::EPSILON);
        assert!((saved_trade.onchain_io_ratio.unwrap() - 200.0).abs() < f64::EPSILON);
        assert_eq!(saved_trade.schwab_ticker.unwrap(), "AAPL");
        assert_eq!(saved_trade.schwab_instruction.unwrap(), "BUY");
        assert!((saved_trade.schwab_quantity.unwrap() - 5.0).abs() < f64::EPSILON);
        assert_eq!(saved_trade.schwab_price_cents.unwrap(), 20000);
        assert_eq!(saved_trade.status.unwrap(), "PENDING");
    }

    #[tokio::test]
    async fn test_save_to_db_duplicate_fails() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let trade = Trade {
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 456,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 10.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 2000.0,
            onchain_io_ratio: 0.005,
            onchain_price_per_share_cents: 20000,
            schwab_ticker: "BAR".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
            schwab_quantity: 10.0,
        };

        trade.save_to_db(&pool).await.unwrap();

        let duplicate_result = trade.save_to_db(&pool).await;
        assert!(duplicate_result.is_err());

        if let Err(TradeConversionError::Database(sqlx::Error::Database(db_err))) = duplicate_result
        {
            assert!(db_err.message().contains("UNIQUE constraint failed"));
        } else {
            panic!("Expected UNIQUE constraint error, got: {duplicate_result:?}");
        }
    }

    #[tokio::test]
    async fn test_exists_in_db_true() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let tx_hash =
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222");

        sqlx::query!(
            "INSERT INTO trades (tx_hash, log_index, status, created_at) VALUES (?, ?, 'PENDING', datetime('now'))",
            "0x2222222222222222222222222222222222222222222222222222222222222222",
            789_i64
        )
        .execute(&pool)
        .await
        .unwrap();

        let exists = Trade::exists_in_db(&pool, tx_hash, 789).await.unwrap();
        assert!(exists);
    }

    #[tokio::test]
    async fn test_exists_in_db_false() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let tx_hash =
            fixed_bytes!("0x3333333333333333333333333333333333333333333333333333333333333333");

        let exists = Trade::exists_in_db(&pool, tx_hash, 999).await.unwrap();
        assert!(!exists);
    }

    #[tokio::test]
    async fn test_schwab_quantity_calculation_buy() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFTs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 0,
                input_amount: U256::from(500_000_000),
                output_index: 1,
                output_amount: U256::from_str("1250000000000000000").unwrap(),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert!((trade.schwab_quantity - 1.25).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "MSFT");
    }

    #[tokio::test]
    async fn test_schwab_quantity_calculation_sell() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"TSLAs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let cache = SymbolCache::default();

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            order,
            OrderFill {
                input_index: 1,
                input_amount: U256::from_str("2750000000000000000").unwrap(),
                output_index: 0,
                output_amount: U256::from(825_000_000),
            },
            get_test_log(),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.schwab_instruction, SchwabInstruction::Sell);
        assert!((trade.schwab_quantity - 2.75).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "TSLA");
    }
}
