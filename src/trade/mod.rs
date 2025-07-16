use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::transports::{RpcError, TransportErrorKind};
use clap::Parser;
use std::num::ParseFloatError;

use crate::bindings::IOrderBookV4::OrderV3;
use crate::symbol_cache::SymbolCache;

mod clear;
mod take_order;

#[derive(Parser, Debug)]
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

impl serde::Serialize for SchwabInstruction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            SchwabInstruction::Buy => "BUY",
            SchwabInstruction::Sell => "SELL",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Trade {
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    tx_hash: B256,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    log_index: u64,

    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_input_symbol: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_input_amount: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_output_symbol: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_output_amount: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_io_ratio: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_price_per_share_cents: u64,

    #[allow(dead_code)] // TODO: remove this once we store trades in db
    schwab_ticker: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    schwab_instruction: SchwabInstruction,
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

        let (schwab_ticker, schwab_instruction) =
            if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
                let ticker = onchain_output_symbol
                    .strip_suffix("s1")
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Sell)
            } else if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("s1") {
                let ticker = onchain_input_symbol
                    .strip_suffix("s1")
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Buy)
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
            onchain_output_amount / onchain_input_amount
        } else {
            onchain_input_amount / onchain_output_amount
        };

        if onchain_price_per_share_usdc.is_nan() {
            return Ok(None);
        }

        let onchain_price_per_share_cents = (onchain_price_per_share_usdc * 100.0) as u64;

        let trade = Trade {
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
        };

        Ok(Some(trade))
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
    async fn test_try_from_order_and_fill_details_ok_sell_schwab() {
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
                input_amount: U256::from(100000000),
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
            schwab_instruction: SchwabInstruction::Sell,
        };

        assert_eq!(trade, expected_trade);

        // test that the symbol is cached
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let trade = Trade::try_from_order_and_fill_details(
            &cache,
            &provider,
            get_test_order(),
            OrderFill {
                input_index: 0,
                input_amount: U256::from(100000000),
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
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Sell);
        assert_eq!(trade.schwab_ticker, "FOO");
    }

    #[tokio::test]
    async fn test_try_from_order_and_fill_details_ok_buy_schwab() {
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
                output_amount: U256::from(100000000),
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
            schwab_instruction: SchwabInstruction::Buy,
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
                input_amount: U256::from(100000000),
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
                input_amount: U256::from(100000000),
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
                input_amount: U256::from(100000000),
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
                input_amount: U256::from(100000000),
                output_index: 99, // invalid output index
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
                input_amount: U256::from(100000000),
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
                input_amount: U256::from(100000000),
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
            &"FOO".to_string(), // no s1 suffix
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
                input_amount: U256::from(100000000),
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
                input_amount: U256::from(100000000),
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
        // zero amount
        assert_eq!(u256_to_f64(U256::ZERO, 6).unwrap(), 0.0);

        // 18 decimals (st0x-like)
        let amount = U256::from_str("1000000000000000000").unwrap(); // 1.0
        assert!((u256_to_f64(amount, 18).unwrap() - 1.0).abs() < f64::EPSILON);

        // 6 decimals (USDC-like)
        let amount = U256::from(123456789u64); // 123.456789 with 6 decimals
        let expected = 123.456789_f64;
        assert!((u256_to_f64(amount, 6).unwrap() - expected).abs() < f64::EPSILON);
    }
}
