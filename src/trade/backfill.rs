use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use alloy::transports::{RpcError, TransportErrorKind};
use backon::{ExponentialBuilder, Retryable};
use futures_util::{StreamExt, TryStreamExt, future, stream};
use itertools::Itertools;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::{EvmEnv, PartialArbTrade, TradeConversionError};
use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::symbol_cache::SymbolCache;

#[derive(Debug, thiserror::Error)]
pub enum BackfillError {
    #[error("RPC failure during batch processing: {0}")]
    RpcFailure(#[from] RpcError<TransportErrorKind>),
    #[error("Trade conversion error: {0}")]
    TradeConversion(#[from] TradeConversionError),
    #[error(
        "Batch processing failed after all retries for range {start_block}-{end_block}: {source}"
    )]
    BatchRetryExhausted {
        start_block: u64,
        end_block: u64,
        source: Box<BackfillError>,
    },
}

const BACKFILL_BATCH_SIZE: usize = 1000;
const BACKFILL_MAX_RETRIES: usize = 3;
const BACKFILL_INITIAL_DELAY: Duration = Duration::from_millis(1000);
const BACKFILL_MAX_DELAY: Duration = Duration::from_secs(30);

pub async fn backfill_events<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
) -> Result<Vec<PartialArbTrade>, BackfillError> {
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

pub async fn process_batch<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
) -> Result<Vec<PartialArbTrade>, BackfillError> {
    let retry_strategy = ExponentialBuilder::default()
        .with_max_times(BACKFILL_MAX_RETRIES)
        .with_min_delay(BACKFILL_INITIAL_DELAY)
        .with_max_delay(BACKFILL_MAX_DELAY);

    let process_batch_inner =
        || async { process_batch_inner(provider, cache, evm_env, batch_start, batch_end).await };

    match process_batch_inner.retry(&retry_strategy).await {
        Ok(result) => Ok(result),
        Err(error) => {
            warn!(
                "Batch processing failed after {} retries for range {}-{}: {}",
                BACKFILL_MAX_RETRIES, batch_start, batch_end, error
            );

            Err(BackfillError::BatchRetryExhausted {
                start_block: batch_start,
                end_block: batch_end,
                source: Box::new(error),
            })
        }
    }
}

async fn process_batch_inner<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
) -> Result<Vec<PartialArbTrade>, BackfillError> {
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

pub fn generate_batch_ranges(start_block: u64, end_block: u64) -> Vec<(u64, u64)> {
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
    use crate::symbol_cache::SymbolCache;
    use crate::test_utils::get_test_order;
    use crate::trade::{EvmEnv, PartialArbTrade, SchwabInstruction};

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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[test]
    fn test_generate_batch_ranges_single_batch() {
        let ranges = generate_batch_ranges(100, 500);
        assert_eq!(ranges, vec![(100, 500)]);
    }

    #[test]
    fn test_generate_batch_ranges_exact_batch_size() {
        let ranges = generate_batch_ranges(100, 1099);
        assert_eq!(ranges, vec![(100, 1099)]);
    }

    #[test]
    fn test_generate_batch_ranges_multiple_batches() {
        let ranges = generate_batch_ranges(100, 2500);
        assert_eq!(ranges, vec![(100, 1099), (1100, 2099), (2100, 2500)]);
    }

    #[test]
    fn test_generate_batch_ranges_single_block() {
        let ranges = generate_batch_ranges(42, 42);
        assert_eq!(ranges, vec![(42, 42)]);
    }

    #[test]
    fn test_generate_batch_ranges_empty() {
        let ranges = generate_batch_ranges(100, 99);
        assert_eq!(ranges.len(), 0);
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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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

        let result = backfill_events(&provider, &cache, &evm_env).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), BackfillError::RpcFailure(_)));
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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }

    fn create_test_take_event(
        order: &crate::bindings::IOrderBookV4::OrderV3,
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
        orderbook: alloy::primitives::Address,
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
        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 2);

        assert_trade_details(&trades[0], tx_hash1, 100.0, 1.0, 1.0, 10000.0);
        assert_trade_details(&trades[1], tx_hash2, 200.0, 2.0, 2.0, 10000.0);
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
        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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
        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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

        let trades = process_batch(&provider, &cache, &evm_env, 100, 200)
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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

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

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }
}
