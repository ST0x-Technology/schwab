use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use alloy::transports::{RpcError, TransportErrorKind};
use clap::Parser;
use futures_util::{StreamExt, TryStreamExt, future, stream};
use itertools::Itertools;
use std::num::ParseFloatError;
use tracing::{debug, info};

use crate::bindings::IOrderBookV4::{ClearV2, OrderV3, TakeOrderV2};
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
    #[clap(short = 'd', long, env)]
    pub deployment_block: u64,
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
pub struct PartialArbTrade {
    pub tx_hash: B256,
    pub log_index: u64,
    pub block_number: u64,

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

const BACKFILL_BATCH_SIZE: usize = 1000;

pub async fn backfill_events<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
) -> Result<Vec<PartialArbTrade>, TradeConversionError> {
    let current_block = provider.get_block_number().await?;
    let total_blocks = current_block - evm_env.deployment_block + 1;

    info!(
        "Starting backfill from block {} to {} ({} blocks)",
        evm_env.deployment_block, current_block, total_blocks
    );

    let batch_ranges = generate_batch_ranges(evm_env.deployment_block, current_block);

    let all_trades: Vec<PartialArbTrade> = stream::iter(batch_ranges.into_iter().enumerate())
        .then(|(batch_index, (batch_start, batch_end))| async move {
            let processed_blocks = batch_index * BACKFILL_BATCH_SIZE;

            debug!(
                "Processing blocks {batch_start}-{batch_end} ({processed_blocks}/{total_blocks} blocks processed)"
            );

            process_batch(provider, cache, evm_env, batch_start, batch_end).await
        })
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .sorted_by_key(|trade| (trade.block_number, trade.log_index))
        .collect();

    info!(
        "Backfill completed: {} valid trades found",
        all_trades.len()
    );

    Ok(all_trades)
}

async fn process_batch<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
) -> Result<Vec<PartialArbTrade>, TradeConversionError> {
    let clear_filter = Filter::new()
        .address(evm_env.orderbook)
        .from_block(batch_start)
        .to_block(batch_end)
        .event_signature(ClearV2::SIGNATURE_HASH);

    let take_filter = Filter::new()
        .address(evm_env.orderbook)
        .from_block(batch_start)
        .to_block(batch_end)
        .event_signature(TakeOrderV2::SIGNATURE_HASH);

    let (clear_logs, take_logs) = future::try_join(
        provider.get_logs(&clear_filter),
        provider.get_logs(&take_filter),
    )
    .await?;

    debug!(
        "Found {} ClearV2 events and {} TakeOrderV2 events in batch {}-{}",
        clear_logs.len(),
        take_logs.len(),
        batch_start,
        batch_end
    );

    let trades = stream::iter(clear_logs.into_iter().chain(take_logs))
        .then(|log| async move {
            if let Ok(clear_event_log) = log.log_decode::<ClearV2>() {
                PartialArbTrade::try_from_clear_v2(
                    evm_env,
                    cache,
                    provider.clone(),
                    clear_event_log.data().clone(),
                    log,
                )
                .await
            } else if let Ok(take_event_log) = log.log_decode::<TakeOrderV2>() {
                PartialArbTrade::try_from_take_order_if_target_order(
                    cache,
                    provider.clone(),
                    take_event_log.data().clone(),
                    log,
                    evm_env.order_hash,
                )
                .await
            } else {
                Ok(None)
            }
        })
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .collect();

    Ok(trades)
}

fn generate_batch_ranges(start_block: u64, end_block: u64) -> Vec<(u64, u64)> {
    (start_block..=end_block)
        .step_by(BACKFILL_BATCH_SIZE)
        .map(|batch_start| {
            let batch_end = (batch_start + u64::try_from(BACKFILL_BATCH_SIZE).unwrap_or(u64::MAX)
                - 1)
            .min(end_block);
            (batch_start, batch_end)
        })
        .collect()
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
        let block_number = log
            .block_number
            .ok_or(TradeConversionError::NoBlockNumber)?;

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
            onchain_output_amount
        } else {
            onchain_input_amount
        };

        let trade = Self {
            tx_hash,
            log_index,
            block_number,

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
    use alloy::primitives::{FixedBytes, IntoLogData, U256, address, fixed_bytes, keccak256};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::rpc::types::Log;
    use alloy::sol_types::{SolCall, SolValue};
    use std::str::FromStr;

    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4;
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
            block_number: 12345,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 100.0,
            onchain_output_symbol: "FOOs1".to_string(),
            onchain_output_amount: 9.0,
            onchain_io_ratio: 100.0 / 9.0,
            onchain_price_per_share_cents: (100.0 / 9.0) * 100.0,
            schwab_ticker: "FOO".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 9.0,
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
            block_number: 12345,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 9.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 100.0,
            onchain_io_ratio: 9.0 / 100.0,
            onchain_price_per_share_cents: (100.0 / 9.0) * 100.0,
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
    async fn test_try_from_order_and_fill_details_err_missing_block_number() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let order = get_test_order();
        let mut log = get_test_log();
        log.block_number = None;
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

        assert!(matches!(err, TradeConversionError::NoBlockNumber));
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
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);
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
        assert!((trade.schwab_quantity - 2.75).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "TSLA");
    }

    #[tokio::test]
    async fn test_backfill_events_empty_results() {
        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_with_clear_v2_events() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_config = IOrderBookV4::ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = IOrderBookV4::ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = IOrderBookV4::AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: IOrderBookV4::ClearStateChange {
                aliceOutput: U256::from_str("9000000000000000000").unwrap(),
                bobOutput: U256::from(100_000_000u64),
                aliceInput: U256::from(100_000_000u64),
                bobInput: U256::from_str("9000000000000000000").unwrap(),
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([clear_log])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events (empty)
        asserter.push_success(&serde_json::json!([after_clear_log])); // AfterClear event lookup
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 1);
        let trade = &trades[0];
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
        assert_eq!(trade.onchain_input_symbol, "USDC");
        assert_eq!(trade.onchain_output_symbol, "AAPLs1");
        assert!((trade.onchain_input_amount - 100.0).abs() < f64::EPSILON);
        assert!((trade.onchain_output_amount - 9.0).abs() < f64::EPSILON);
        assert!((trade.onchain_io_ratio - (100.0 / 9.0)).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "AAPL");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);

        let expected_price = (100.0_f64 / 9.0) * 100.0;
        assert!(
            (trade.onchain_price_per_share_cents - expected_price).abs() < f64::EPSILON,
            "Expected price {} but got {}",
            expected_price,
            trade.onchain_price_per_share_cents
        );
    }

    #[tokio::test]
    async fn test_backfill_events_with_take_order_v2_events() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        let take_event = IOrderBookV4::TakeOrderV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            config: IOrderBookV4::TakeOrderConfigV3 {
                order: order.clone(),
                inputIOIndex: U256::from(0),
                outputIOIndex: U256::from(1),
                signedContext: Vec::new(),
            },
            input: U256::from(100_000_000),
            output: U256::from_str("9000000000000000000").unwrap(),
        };

        let take_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: take_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events (empty)
        asserter.push_success(&serde_json::json!([take_log])); // take events
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFTs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 1);
        let trade = &trades[0];
        assert_eq!(
            trade.tx_hash,
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        );
        assert_eq!(trade.log_index, 1);
        assert_eq!(trade.onchain_input_symbol, "USDC");
        assert_eq!(trade.onchain_output_symbol, "MSFTs1");
        assert!((trade.onchain_input_amount - 100.0).abs() < f64::EPSILON);
        assert!((trade.onchain_output_amount - 9.0).abs() < f64::EPSILON);
        assert!((trade.onchain_io_ratio - (100.0 / 9.0)).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "MSFT");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert!((trade.schwab_quantity - 9.0).abs() < f64::EPSILON);

        let expected_price = (100.0_f64 / 9.0) * 100.0;
        assert!(
            (trade.onchain_price_per_share_cents - expected_price).abs() < f64::EPSILON,
            "Expected price {} but got {}",
            expected_price,
            trade.onchain_price_per_share_cents
        );
    }

    #[tokio::test]
    async fn test_backfill_events_filters_non_matching_orders() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let different_order = get_test_order();
        let clear_event = IOrderBookV4::ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: different_order.clone(),
            bob: different_order.clone(),
            clearConfig: IOrderBookV4::ClearConfig {
                aliceInputIOIndex: U256::from(0),
                aliceOutputIOIndex: U256::from(1),
                bobInputIOIndex: U256::from(1),
                bobOutputIOIndex: U256::from(0),
                aliceBountyVaultId: U256::ZERO,
                bobBountyVaultId: U256::ZERO,
            },
        };

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([clear_log])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events (empty)

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_handles_rpc_errors() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_failure_msg("RPC error");

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = super::backfill_events(&provider, &cache, &evm_env).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TradeConversionError::RpcTransport(_)
        ));
    }

    #[tokio::test]
    async fn test_backfill_events_block_range() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 50,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 0);
    }

    fn create_test_take_event(
        order: &OrderV3,
        input: u64,
        output: &str,
    ) -> IOrderBookV4::TakeOrderV2 {
        IOrderBookV4::TakeOrderV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            config: IOrderBookV4::TakeOrderConfigV3 {
                order: order.clone(),
                inputIOIndex: U256::from(0),
                outputIOIndex: U256::from(1),
                signedContext: Vec::new(),
            },
            input: U256::from(input),
            output: U256::from_str(output).unwrap(),
        }
    }

    fn create_test_log(
        orderbook: Address,
        event: &IOrderBookV4::TakeOrderV2,
        block_number: u64,
        tx_hash: FixedBytes<32>,
    ) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(block_number),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        }
    }

    fn assert_trade_details(
        trade: &PartialArbTrade,
        tx_hash: FixedBytes<32>,
        input_amount: f64,
        output_amount: f64,
        schwab_quantity: f64,
        price_per_share_cents: f64,
    ) {
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
        assert_eq!(trade.onchain_input_symbol, "USDC");
        assert_eq!(trade.onchain_output_symbol, "MSFTs1");
        assert!((trade.onchain_input_amount - input_amount).abs() < f64::EPSILON);
        assert!((trade.onchain_output_amount - output_amount).abs() < f64::EPSILON);
        assert!((trade.schwab_quantity - schwab_quantity).abs() < f64::EPSILON);
        assert_eq!(trade.schwab_ticker, "MSFT");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
        assert!((trade.onchain_price_per_share_cents - price_per_share_cents).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_backfill_events_preserves_chronological_order() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        let tx_hash1 =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111");
        let tx_hash2 =
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222");

        let take_event1 = create_test_take_event(&order, 100_000_000, "1000000000000000000");
        let take_event2 = create_test_take_event(&order, 200_000_000, "2000000000000000000");

        let take_log1 = create_test_log(evm_env.orderbook, &take_event1, 50, tx_hash1);
        let take_log2 = create_test_log(evm_env.orderbook, &take_event2, 100, tx_hash2);

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(200u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([take_log2, take_log1]));

        // Symbol calls for both trades
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFTs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFTs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();
        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 2);

        assert_trade_details(&trades[0], tx_hash1, 100.0, 1.0, 1.0, 10000.0);
        assert_trade_details(&trades[1], tx_hash2, 200.0, 2.0, 2.0, 10000.0);
    }

    #[test]
    fn test_generate_batch_ranges_single_batch() {
        let ranges = super::generate_batch_ranges(100, 500);
        assert_eq!(ranges, vec![(100, 500)]);
    }

    #[test]
    fn test_generate_batch_ranges_exact_batch_size() {
        let ranges = super::generate_batch_ranges(100, 1099);
        assert_eq!(ranges, vec![(100, 1099)]);
    }

    #[test]
    fn test_generate_batch_ranges_multiple_batches() {
        let ranges = super::generate_batch_ranges(100, 2500);
        assert_eq!(ranges, vec![(100, 1099), (1100, 2099), (2100, 2500)]);
    }

    #[test]
    fn test_generate_batch_ranges_single_block() {
        let ranges = super::generate_batch_ranges(42, 42);
        assert_eq!(ranges, vec![(42, 42)]);
    }

    #[test]
    fn test_generate_batch_ranges_empty() {
        let ranges = super::generate_batch_ranges(100, 99);
        assert_eq!(ranges.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_batch_count_verification() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1000,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(2500u64));

        // Batch 1: blocks 1000-1999
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        // Batch 2: blocks 2000-2500
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();
        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        // Verifies that batching correctly handles the expected number of RPC calls
        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_batch_boundary_verification() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 500,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(1900u64));

        // Batch 1: blocks 500-1499
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        // Batch 2: blocks 1500-1900
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();
        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        // Verify the batching worked correctly for different deployment/current block combination
        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_process_batch_with_realistic_data() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        let tx_hash =
            fixed_bytes!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd");
        let take_event = create_test_take_event(&order, 500_000_000, "5000000000000000000");
        let take_log = create_test_log(evm_env.orderbook, &take_event, 150, tx_hash);

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([take_log]));

        // Symbol calls
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFTs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::process_batch(&provider, &cache, &evm_env, 100, 200)
            .await
            .unwrap();

        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].tx_hash, tx_hash);
        assert_eq!(trades[0].block_number, 150);
        assert!((trades[0].schwab_quantity - 5.0).abs() < f64::EPSILON);
        assert_eq!(trades[0].schwab_ticker, "MSFT");
    }

    #[tokio::test]
    async fn test_backfill_events_deployment_equals_current_block() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 100,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_large_block_range_batching() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(3000u64));

        for _ in 0..6 {
            asserter.push_success(&serde_json::json!([]));
            asserter.push_success(&serde_json::json!([]));
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = super::backfill_events(&provider, &cache, &evm_env)
            .await
            .unwrap();

        assert_eq!(trades.len(), 0);
    }
}
