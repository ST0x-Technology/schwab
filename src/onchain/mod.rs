use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::transports::{RpcError, TransportErrorKind};
use clap::Parser;
use std::num::ParseFloatError;

use crate::bindings::IOrderBookV4::OrderV3;
use crate::schwab::SchwabInstruction;
use crate::symbol_cache::SymbolCache;

mod clear;
pub mod coordinator;
mod position_accumulator;
mod processor;
mod take_order;
pub mod trade;
pub mod trade_executions;

pub use coordinator::TradeCoordinator;
pub use position_accumulator::{ExecutablePosition, PositionAccumulator, accumulate_onchain_trade};
pub use trade::{OnchainTrade, OnchainTradeStatus};
pub use trade_executions::TradeExecutionLink;

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
pub enum TradeStatus {
    Pending,
    Completed,
    Failed,
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
pub struct PartialArbTrade {
    pub tx_hash: B256,
    pub log_index: u64,

    pub onchain_input_symbol: String,
    // TODO: Consider migrating to rust_decimal::Decimal or integer base units for exact precision
    // Current f64 may lose precision for 18-decimal tokenized stocks (USDC=6 decimals, stocks=18 decimals)
    // Will need to change for V5 orderbook upgrade (custom Float types)
    pub onchain_input_amount: f64,
    pub onchain_output_symbol: String,
    pub onchain_output_amount: f64,
    pub onchain_io_ratio: f64,
    pub onchain_price_per_share_cents: f64,

    pub schwab_ticker: String,
    pub schwab_instruction: SchwabInstruction,
    pub schwab_quantity: u64,
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
    #[error("Transaction not found: {0}")]
    TransactionNotFound(B256),
    #[error("Invalid Schwab instruction in database: {0}")]
    InvalidSchwabInstruction(String),
    #[error("Invalid trade status in database: {0}")]
    InvalidTradeStatus(String),
}

struct OrderFill {
    input_index: usize,
    input_amount: U256,
    output_index: usize,
    output_amount: U256,
}

impl PartialArbTrade {
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
                    .map(ToString::to_string)
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
                    .map(ToString::to_string)
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

        let onchain_price_per_share_cents = onchain_price_per_share_usdc * 100.0;

        let schwab_quantity = if schwab_instruction == SchwabInstruction::Buy {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let quantity = onchain_output_amount.round() as u64;
            quantity
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let quantity = onchain_input_amount.round() as u64;
            quantity
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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

        let expected_trade = PartialArbTrade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 100.0,
            onchain_output_symbol: "FOOs1".to_string(),
            onchain_output_amount: 9.0,
            onchain_io_ratio: 100.0 / 9.0,
            onchain_price_per_share_cents: (100.0 / 9.0) * 100.0,
            schwab_ticker: "FOO".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 9,
        };

        assert_eq!(trade, expected_trade);

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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
        assert_eq!(trade.schwab_quantity, 9);
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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

        let expected_trade = PartialArbTrade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 9.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 100.0,
            onchain_io_ratio: 9.0 / 100.0,
            onchain_price_per_share_cents: (100.0 / 9.0) * 100.0,
            schwab_ticker: "BAR".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
            schwab_quantity: 9,
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let err = PartialArbTrade::try_from_order_and_fill_details(
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

        let result = PartialArbTrade::try_from_order_and_fill_details(
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
        assert!(result.onchain_price_per_share_cents.is_infinite());
        assert_eq!(result.schwab_quantity, 0);
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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
        assert_eq!(trade.schwab_quantity, 9);
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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
        assert_eq!(trade.schwab_quantity, 9);
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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
        assert_eq!(trade.schwab_quantity, 1);
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

        let trade = PartialArbTrade::try_from_order_and_fill_details(
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
        assert_eq!(trade.schwab_quantity, 3);
        assert_eq!(trade.schwab_ticker, "TSLA");
    }
}
