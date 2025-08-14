use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use chrono::{DateTime, Utc};
use clap::Parser;
use std::num::ParseFloatError;

use crate::bindings::IOrderBookV4::OrderV3;
use crate::error::{OnChainError, TradeValidationError};
use crate::schwab::SchwabInstruction;
use crate::symbol_cache::SymbolCache;

mod clear;
pub mod position_calculator;
mod processor;
mod take_order;
pub mod trade;

pub use trade::OnchainTrade;

#[derive(Parser, Debug, Clone)]
pub struct EvmEnv {
    #[clap(short, long, env)]
    pub ws_rpc_url: url::Url,
    #[clap(short = 'b', long, env)]
    pub orderbook: Address,
    #[clap(short, long, env)]
    pub order_hash: B256,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeStatus {
    Pending,
    Completed {
        executed_at: DateTime<Utc>,
        order_id: String,
        price_cents: u64,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error_reason: Option<String>,
    },
}

impl TradeStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Completed { .. } => "COMPLETED",
            Self::Failed { .. } => "FAILED",
        }
    }
}

impl std::str::FromStr for TradeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PENDING" => Ok(Self::Pending),
            "COMPLETED" => Err("Cannot create Completed status without required data".to_string()),
            "FAILED" => Err("Cannot create Failed status without required data".to_string()),
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

        let (schwab_ticker, schwab_instruction) =
            Self::determine_schwab_trade_details(&onchain_input_symbol, &onchain_output_symbol)?;

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
            shares_from_amount(onchain_output_amount)
        } else {
            shares_from_amount(onchain_input_amount)
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

    /// Determines Schwab trade direction and ticker based on onchain symbol configuration.
    ///
    /// If the on-chain order has USDC as input and an s1 tokenized stock as
    /// output then it means the order received USDC and gave away an s1
    /// tokenized stock, i.e. sold, which means that to take the opposite
    /// trade in schwab we need to buy and vice versa.
    fn determine_schwab_trade_details(
        onchain_input_symbol: &str,
        onchain_output_symbol: &str,
    ) -> Result<(String, SchwabInstruction), OnChainError> {
        // USDC input + s1 tokenized stock output = sold tokenized stock = buy on Schwab
        if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
            let ticker = Self::extract_ticker_from_s1_symbol(
                onchain_output_symbol,
                onchain_input_symbol,
                onchain_output_symbol,
            )?;
            return Ok((ticker, SchwabInstruction::Buy));
        }

        // s1 tokenized stock input + USDC output = bought tokenized stock = sell on Schwab
        if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("s1") {
            let ticker = Self::extract_ticker_from_s1_symbol(
                onchain_input_symbol,
                onchain_input_symbol,
                onchain_output_symbol,
            )?;
            return Ok((ticker, SchwabInstruction::Sell));
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

/// Converts a fractional token amount to whole share count for Schwab execution.
///
/// Financial context: Schwab only accepts integer share quantities, but onchain
/// trades can involve fractional amounts. This function rounds to nearest share
/// and handles the precision loss explicitly.
fn shares_from_amount(amount: f64) -> u64 {
    // Precision loss is acceptable here because:
    // 1. Schwab API only accepts whole shares
    // 2. Fractional shares are accumulated separately in TradeAccumulator
    // 3. Rounding to nearest prevents systematic bias
    if amount < 0.0 {
        0 // Negative amounts result in 0 shares (shouldn't happen in normal flow)
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            amount.round() as u64 // Safe: round() removes fractional part, negative case handled above
        }
    }
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

        assert!(matches!(
            err,
            OnChainError::Validation(TradeValidationError::NoTxHash)
        ));
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

        assert!(matches!(
            err,
            OnChainError::Validation(TradeValidationError::NoLogIndex)
        ));
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

        assert!(matches!(
            err,
            OnChainError::Validation(TradeValidationError::NoInputAtIndex(99))
        ));
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

        assert!(matches!(
            err,
            OnChainError::Validation(TradeValidationError::NoOutputAtIndex(99))
        ));
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

        assert!(matches!(
            err,
            OnChainError::Execution(crate::error::ExecutionError::GetSymbol(_))
        ));
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
            matches!(err, OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(ref input, ref output)) if input == "USDC" && output == "USDC")
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
            matches!(err, OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(ref input, ref output)) if input == "USDC" && output == "FOO")
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
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(ref input, ref output))
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
