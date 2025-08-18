use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use backon::{ExponentialBuilder, Retryable};
use futures_util::future;
use itertools::Itertools;
use sqlx::SqlitePool;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::EvmEnv;
use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::error::OnChainError;
use crate::queue::{enqueue, get_unprocessed_count};

#[derive(Debug, Clone)]
enum DecodedEvent {
    ClearV2(ClearV2),
    TakeOrderV2(TakeOrderV2),
}

const BACKFILL_BATCH_SIZE: usize = 1000;
const BACKFILL_MAX_RETRIES: usize = 3;
const BACKFILL_INITIAL_DELAY: Duration = Duration::from_millis(1000);
const BACKFILL_MAX_DELAY: Duration = Duration::from_secs(30);

pub async fn backfill_events<P: Provider + Clone>(
    pool: &SqlitePool,
    provider: &P,
    evm_env: &EvmEnv,
    end_block: u64,
) -> Result<(), OnChainError> {
    if evm_env.deployment_block > end_block {
        return Ok(());
    }

    let total_blocks = end_block - evm_env.deployment_block + 1;

    info!(
        "Starting backfill from block {} to {} ({} blocks)",
        evm_env.deployment_block, end_block, total_blocks
    );

    let batch_ranges = generate_batch_ranges(evm_env.deployment_block, end_block);

    let batch_tasks = batch_ranges
        .into_iter()
        .enumerate()
        .map(|(batch_index, (batch_start, batch_end))| {
            let processed_blocks = batch_index * BACKFILL_BATCH_SIZE;

            debug!(
                "Processing blocks {batch_start}-{batch_end} ({processed_blocks}/{total_blocks} blocks processed)"
            );

            enqueue_batch_events(pool, provider, evm_env, batch_start, batch_end)
        })
        .collect::<Vec<_>>();

    let batch_results = future::join_all(batch_tasks).await;

    let total_enqueued = batch_results
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<usize>();

    info!("Backfill completed: {total_enqueued} events enqueued");

    Ok(())
}

pub async fn process_batch<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
) -> Result<Vec<OnchainTrade>, OnChainError> {
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

            Err(error)
        }
    }
}

async fn process_batch_inner<P: Provider + Clone>(
    provider: &P,
    cache: &SymbolCache,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
) -> Result<Vec<OnchainTrade>, OnChainError> {
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

    let all_logs = clear_logs
        .into_iter()
        .chain(take_logs.into_iter())
        .collect::<Vec<_>>();

    let enqueue_tasks = all_logs
        .into_iter()
        .sorted_by_key(|log| (log.block_number, log.log_index))
        .filter_map(|log| {
            // Try ClearV2 first
            if let Ok(clear_event) = log.log_decode::<ClearV2>() {
                Some((
                    DecodedEvent::ClearV2(Box::new(clear_event.data().clone())),
                    log,
                ))
            // Then try TakeOrderV2
            } else if let Ok(take_event) = log.log_decode::<TakeOrderV2>() {
                Some((
                    DecodedEvent::TakeOrderV2(Box::new(take_event.data().clone())),
                    log,
                ))
            } else {
                None
            }
        })
        .map(|(decoded_event, log)| async move {
            let result = match decoded_event {
                DecodedEvent::ClearV2(event) => enqueue(pool, &*event, &log).await,
                DecodedEvent::TakeOrderV2(event) => enqueue(pool, &*event, &log).await,
            };
            match result {
                Ok(()) => Some(()),
                Err(e) => {
                    warn!("Failed to enqueue event during backfill: {e}");
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    let enqueue_results = future::join_all(enqueue_tasks).await;

    let enqueued_count = enqueue_results.into_iter().flatten().count();

    Ok(enqueued_count)
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
    use crate::onchain::{EvmEnv, trade::OnchainTrade};
    use crate::schwab::Direction;
    use crate::symbol_cache::SymbolCache;
    use crate::test_utils::get_test_order;

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
        let pool = setup_test_db().await;
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
        assert_eq!(trade.symbol, "AAPLs1");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(trade.direction, Direction::Buy);

        let expected_price = 100.0_f64 / 9.0;
        assert!(
            (trade.price_usdc - expected_price).abs() < f64::EPSILON,
            "Expected price {} but got {}",
            expected_price,
            trade.price_usdc
        );
    }

    #[tokio::test]
    async fn test_backfill_events_with_take_order_v2_events() {
        let pool = setup_test_db().await;
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

        let tx_hash = fixed_bytes!(
            "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        );
        let take_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: take_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
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
        assert_eq!(trade.symbol, "MSFTs1");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(trade.direction, Direction::Buy);

        let expected_price = 100.0_f64 / 9.0;
        assert!(
            (trade.price_usdc - expected_price).abs() < f64::EPSILON,
            "Expected price {} but got {}",
            expected_price,
            trade.price_usdc
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
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
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
        trade: &OnchainTrade,
        tx_hash: FixedBytes<32>,
        trade_amount: f64,
        price_usdc: f64,
    ) {
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
        assert_eq!(trade.symbol, "MSFTs1");
        assert!((trade.amount - trade_amount).abs() < f64::EPSILON);
        assert_eq!(trade.direction, Direction::Buy);
        assert!((trade.price_usdc - price_usdc).abs() < f64::EPSILON);
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

        assert_trade_details(&trades[0], tx_hash1, 1.0, 100.0);
        assert_trade_details(&trades[1], tx_hash2, 2.0, 100.0);
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
        assert!((trades[0].amount - 5.0).abs() < f64::EPSILON);
        assert_eq!(trades[0].symbol, "MSFTs1");
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

    #[tokio::test]
    async fn test_backfill_and_process_trades_accumulation() {
        use crate::test_utils::setup_test_db;

        let pool = setup_test_db().await;

        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        // Create multiple take order events that should accumulate
        let take_event1 = create_test_take_event(&order, 30_000_000, "300000000000000000"); // 0.3 shares
        let take_event2 = create_test_take_event(&order, 40_000_000, "400000000000000000"); // 0.4 shares  
        let take_event3 = create_test_take_event(&order, 50_000_000, "500000000000000000"); // 0.5 shares

        let take_log1 = create_test_log(
            evm_env.orderbook,
            &take_event1,
            50,
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
        );
        let take_log2 = create_test_log(
            evm_env.orderbook,
            &take_event2,
            51,
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222"),
        );
        let take_log3 = create_test_log(
            evm_env.orderbook,
            &take_event3,
            52,
            fixed_bytes!("0x3333333333333333333333333333333333333333333333333333333333333333"),
        );

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events (empty)
        asserter.push_success(&serde_json::json!([take_log1, take_log2, take_log3])); // take events

        // Symbol calls for all three trades
        for _ in 0..6 {
            asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
                &"USDC".to_string(),
            ));
            asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
                &"MSFTs1".to_string(),
            ));
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let executions = backfill_and_process_trades(&pool, &provider, &cache, &evm_env)
            .await
            .unwrap();

        // Should trigger one execution when accumulated trades reach >= 1.0 share
        assert_eq!(executions.len(), 1);
        let execution = &executions[0];

        assert_eq!(execution.symbol, "MSFT");
        assert_eq!(execution.shares, 1);
        assert_eq!(execution.direction, Direction::Buy);

        // Verify accumulator state shows remaining fractional amount
        let (calculator, _) = accumulator::find_by_symbol(&pool, "MSFT")
            .await
            .unwrap()
            .unwrap();
        // Total: 0.3 + 0.4 + 0.5 = 1.2, executed 1.0, remaining 0.2
        assert!((calculator.accumulated_long - 0.2).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_backfill_events_deployment_after_current_block() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 200,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_mixed_valid_and_invalid_events() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash,
            deployment_block: 1,
        };

        let valid_take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");

        // Create different order with different hash to make it invalid
        let mut different_order = get_test_order();
        different_order.nonce =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"); // Change nonce to make hash different
        let invalid_take_event =
            create_test_take_event(&different_order, 50_000_000, "5000000000000000000");

        let valid_log = create_test_log(
            evm_env.orderbook,
            &valid_take_event,
            50,
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
        );
        let invalid_log = create_test_log(
            evm_env.orderbook,
            &invalid_take_event,
            51,
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222"),
        );

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([valid_log, invalid_log]));

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
        assert_eq!(
            trades[0].tx_hash,
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111")
        );
    }

    #[tokio::test]
    async fn test_backfill_events_mixed_clear_and_take_events() {
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

        let take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");
        let take_log = create_test_log(evm_env.orderbook, &take_event, 50, tx_hash1);

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
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(tx_hash2),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = IOrderBookV4::AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: IOrderBookV4::ClearStateChange {
                aliceOutput: U256::from_str("5000000000000000000").unwrap(), // 5 shares
                bobOutput: U256::from(50_000_000u64),                        // 50 USDC cents
                aliceInput: U256::from(50_000_000u64),
                bobInput: U256::from_str("5000000000000000000").unwrap(),
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: evm_env.orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(tx_hash2),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(150u64)); // 1. get_block_number
        asserter.push_success(&serde_json::json!([clear_log])); // 2. get_logs clear
        asserter.push_success(&serde_json::json!([take_log])); // 3. get_logs take

        // Take event processing (processed first due to earlier block)
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 4. symbol input
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 5. symbol output
            &"AAPLs1".to_string(),
        ));

        // Clear event processing (processed second due to later block)
        asserter.push_success(&serde_json::json!([after_clear_log])); // 6. get_logs AfterClear
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 7. symbol input
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 8. symbol output
            &"AAPLs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 2);

        // Take event should be first (earlier block number)
        assert_eq!(trades[0].tx_hash, tx_hash1);
        assert_eq!(trades[0].symbol, "AAPLs1");
        assert!((trades[0].amount - 9.0).abs() < f64::EPSILON);

        // Clear event should be second (later block number)
        assert_eq!(trades[1].tx_hash, tx_hash2);
        assert_eq!(trades[1].symbol, "AAPLs1");
        assert!((trades[1].amount - 5.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_process_batch_retry_mechanism() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        // First two calls fail, third succeeds
        asserter.push_failure_msg("RPC connection error");
        asserter.push_failure_msg("Timeout error");
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = process_batch(&provider, &cache, &evm_env, 100, 200).await;

        assert!(result.is_ok());
        let trades = result.unwrap();
        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_process_batch_exhausted_retries() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        // All retry attempts fail
        for _ in 0..=BACKFILL_MAX_RETRIES {
            asserter.push_failure_msg("Persistent RPC error");
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = process_batch(&provider, &cache, &evm_env, 100, 200).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_partial_batch_failure() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(2500u64));

        // First batch succeeds
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        // Second batch fails completely (after retries)
        for _ in 0..=BACKFILL_MAX_RETRIES {
            asserter.push_failure_msg("Network failure");
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = backfill_events(&provider, &cache, &evm_env).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_corrupted_log_data() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 1,
        };

        // Create malformed log with invalid event signature
        let corrupted_log = Log {
            inner: alloy::primitives::Log::new(
                evm_env.orderbook,
                Vec::new(),
                Vec::from([0x00u8; 32]).into(),
            )
            .unwrap(),
            block_hash: None,
            block_number: Some(50),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));
        asserter.push_success(&serde_json::json!([corrupted_log]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }

    #[tokio::test]
    async fn test_backfill_events_single_block_range() {
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            deployment_block: 42,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(42u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trades = backfill_events(&provider, &cache, &evm_env).await.unwrap();

        assert_eq!(trades.len(), 0);
    }
}
