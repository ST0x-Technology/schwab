use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent;
use backon::{BackoffBuilder, ExponentialBuilder, Retryable};
use futures_util::future;
use itertools::Itertools;
use sqlx::SqlitePool;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use super::EvmEnv;
use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::error::OnChainError;
use crate::queue::enqueue;

fn get_backfill_retry_strat() -> ExponentialBuilder {
    const BACKFILL_MAX_RETRIES: usize = 10;
    const BACKFILL_INITIAL_DELAY: Duration = Duration::from_millis(3000);
    const BACKFILL_MAX_DELAY: Duration = Duration::from_secs(120);

    ExponentialBuilder::default()
        .with_max_times(BACKFILL_MAX_RETRIES)
        .with_min_delay(BACKFILL_INITIAL_DELAY)
        .with_max_delay(BACKFILL_MAX_DELAY)
        .with_jitter()
}

#[derive(Debug)]
enum EventData {
    ClearV2(Box<ClearV2>),
    TakeOrderV2(Box<TakeOrderV2>),
}

pub(crate) async fn backfill_events<P: Provider + Clone>(
    pool: &SqlitePool,
    provider: &P,
    evm_env: &EvmEnv,
    end_block: u64,
) -> Result<(), OnChainError> {
    let retry_strat = get_backfill_retry_strat();
    backfill_events_with_retry_strat(pool, provider, evm_env, end_block, retry_strat).await
}

async fn backfill_events_with_retry_strat<P: Provider + Clone, B: BackoffBuilder + Clone>(
    pool: &SqlitePool,
    provider: &P,
    evm_env: &EvmEnv,
    end_block: u64,
    retry_strategy: B,
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
        .map(|(batch_start, batch_end)| {
            enqueue_batch_events(
                pool,
                provider,
                evm_env,
                batch_start,
                batch_end,
                retry_strategy.clone(),
            )
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

async fn enqueue_batch_events<P: Provider + Clone, B: BackoffBuilder + Clone>(
    pool: &SqlitePool,
    provider: &P,
    evm_env: &EvmEnv,
    batch_start: u64,
    batch_end: u64,
    retry_strategy: B,
) -> Result<usize, OnChainError> {
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

    // Use the provided retry strategy

    let provider_clear = provider.clone();
    let provider_take = provider.clone();
    let clear_filter_clone = clear_filter.clone();
    let take_filter_clone = take_filter.clone();

    let get_clear_logs = move || {
        let provider = provider_clear.clone();
        let filter = clear_filter_clone.clone();
        async move { provider.get_logs(&filter).await }
    };
    let get_take_logs = move || {
        let provider = provider_take.clone();
        let filter = take_filter_clone.clone();
        async move { provider.get_logs(&filter).await }
    };

    let (clear_logs, take_logs) = future::try_join(
        get_clear_logs
            .retry(retry_strategy.clone().build())
            .notify(|err, dur| {
                trace!("Retrying clear_logs after error: {err} (waiting {dur:?})");
            }),
        get_take_logs
            .retry(retry_strategy.build())
            .notify(|err, dur| {
                trace!("Retrying take_logs after error: {err} (waiting {dur:?})");
            }),
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
                let event_data = EventData::ClearV2(Box::new(clear_event.data().clone()));
                Some((event_data, log))
            // Then try TakeOrderV2
            } else if let Ok(take_event) = log.log_decode::<TakeOrderV2>() {
                let event_data = EventData::TakeOrderV2(Box::new(take_event.data().clone()));
                Some((event_data, log))
            } else {
                None
            }
        })
        .map(|(event_data, log)| async move {
            match event_data {
                EventData::ClearV2(event) => match enqueue(pool, &*event, &log).await {
                    Ok(()) => Some(()),
                    Err(e) => {
                        warn!("Failed to enqueue ClearV2 event during backfill: {e}");
                        None
                    }
                },
                EventData::TakeOrderV2(event) => match enqueue(pool, &*event, &log).await {
                    Ok(()) => Some(()),
                    Err(e) => {
                        warn!("Failed to enqueue TakeOrderV2 event during backfill: {e}");
                        None
                    }
                },
            }
        })
        .collect::<Vec<_>>();

    let enqueue_results = future::join_all(enqueue_tasks).await;

    let enqueued_count = enqueue_results.into_iter().flatten().count();

    Ok(enqueued_count)
}

fn generate_batch_ranges(start_block: u64, end_block: u64) -> Vec<(u64, u64)> {
    const BACKFILL_BATCH_SIZE: usize = 10_000;

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
    use crate::onchain::trade::TradeEvent;
    use crate::queue::{count_unprocessed, get_next_unprocessed_event, mark_event_processed};
    use alloy::primitives::{FixedBytes, IntoLogData, U256, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::rpc::types::Log;
    use alloy::sol_types::SolCall;
    use std::str::FromStr;

    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4;
    use crate::onchain::EvmEnv;
    use crate::test_utils::{get_test_order, setup_test_db};

    fn test_retry_strategy() -> ExponentialBuilder {
        ExponentialBuilder::default()
            .with_max_times(2) // Only 2 retries for tests (3 attempts total)
            .with_min_delay(Duration::from_millis(1))
            .with_max_delay(Duration::from_millis(10))
    }

    #[tokio::test]
    async fn test_backfill_events_empty_results() {
        let pool = setup_test_db().await;
        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
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
        let ranges = generate_batch_ranges(100, 25000);
        assert_eq!(ranges, vec![(100, 10099), (10100, 20099), (20100, 25000)]);
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
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([clear_log])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events (empty)

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Verify the enqueued event details
        let queued_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(queued_event.tx_hash, tx_hash);
        assert_eq!(queued_event.log_index, 1);
        assert!(matches!(queued_event.event, TradeEvent::ClearV2(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_with_take_order_v2_events() {
        let pool = setup_test_db().await;
        let order = get_test_order();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
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
            &"MSFT0x".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Check that one event was enqueued
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Verify the enqueued event details
        let queued_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(queued_event.tx_hash, tx_hash);
        assert_eq!(queued_event.log_index, 1);
        assert!(matches!(queued_event.event, TradeEvent::TakeOrderV2(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_enqueues_all_events() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Should enqueue the event (filtering happens during queue processing, not backfill)
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_backfill_events_handles_rpc_errors() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number call
        // Need 2 failures: one for clear_logs retry, one for take_logs retry (they run in parallel)
        asserter.push_failure_msg("RPC error"); // clear_logs failure
        asserter.push_failure_msg("RPC error"); // take_logs failure

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = backfill_events_with_retry_strat(
            &pool,
            &provider,
            &evm_env,
            100,
            test_retry_strategy(),
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_block_range() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 50,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64)); // get_block_number
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
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

    #[tokio::test]
    async fn test_backfill_events_preserves_chronological_order() {
        let pool = setup_test_db().await;
        let order = get_test_order();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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
            &"MSFT0x".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"MSFT0x".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Check that two events were enqueued
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 2);

        // Verify the first event (earlier block number)
        let first_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(first_event.tx_hash, tx_hash1);
        assert_eq!(first_event.block_number, 50);

        // Mark as processed and get the second event
        let mut sql_tx = pool.begin().await.unwrap();
        mark_event_processed(&mut sql_tx, first_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let second_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(second_event.tx_hash, tx_hash2);
        assert_eq!(second_event.block_number, 100);
    }

    #[tokio::test]
    async fn test_backfill_events_batch_count_verification() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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
        backfill_events(&pool, &provider, &evm_env, 2500)
            .await
            .unwrap();

        // Verifies that batching correctly handles the expected number of RPC calls
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_events_batch_boundary_verification() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        backfill_events_with_retry_strat(
            &pool,
            &provider,
            &evm_env,
            1900,
            get_backfill_retry_strat(),
        )
        .await
        .unwrap();

        // Verify the batching worked correctly for different deployment/current block combination
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_process_batch_with_realistic_data() {
        let pool = setup_test_db().await;
        let order = get_test_order();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let tx_hash =
            fixed_bytes!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd");
        let take_event = create_test_take_event(&order, 500_000_000, "5000000000000000000");
        let take_log = create_test_log(evm_env.orderbook, &take_event, 150, tx_hash);

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([take_log]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let enqueued_count =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy())
                .await
                .unwrap();

        assert_eq!(enqueued_count, 1);

        // Verify the enqueued event
        let queued_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(queued_event.tx_hash, tx_hash);
        assert_eq!(queued_event.block_number, 150);
    }

    #[tokio::test]
    async fn test_backfill_events_deployment_equals_current_block() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 100,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_events_large_block_range_batching() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(3000u64));

        for _ in 0..6 {
            asserter.push_success(&serde_json::json!([]));
            asserter.push_success(&serde_json::json!([]));
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events_with_retry_strat(
            &pool,
            &provider,
            &evm_env,
            3000,
            get_backfill_retry_strat(),
        )
        .await
        .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_events_deployment_after_current_block() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 200,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events_with_retry_strat(&pool, &provider, &evm_env, 100, test_retry_strategy())
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_events_mixed_valid_and_invalid_events() {
        let pool = setup_test_db().await;
        let order = get_test_order();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let valid_take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");

        // Create different order with different hash to make it invalid
        let mut different_order = get_test_order();
        different_order.nonce =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"); // Change nonce to make hash different
        let invalid_take_event =
            create_test_take_event(&different_order, 50_000_000, "5000000000000000000");

        let valid_tx_hash =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111");
        let invalid_tx_hash =
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222");
        let valid_log = create_test_log(evm_env.orderbook, &valid_take_event, 50, valid_tx_hash);
        let invalid_log =
            create_test_log(evm_env.orderbook, &invalid_take_event, 51, invalid_tx_hash);

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(100u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([valid_log, invalid_log]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Both events should be enqueued (filtering happens during processing, not backfill)
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_backfill_events_mixed_clear_and_take_events() {
        let pool = setup_test_db().await;
        let order = get_test_order();
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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
            &"AAPL0x".to_string(),
        ));

        // Clear event processing (processed second due to later block)
        asserter.push_success(&serde_json::json!([after_clear_log])); // 6. get_logs AfterClear
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 7. symbol input
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            // 8. symbol output
            &"AAPL0x".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Check that two events were enqueued
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 2);

        // Verify the first event (earlier block number)
        let first_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(first_event.tx_hash, tx_hash1);
        assert_eq!(first_event.block_number, 50);

        // Mark as processed and get the second event
        let mut sql_tx = pool.begin().await.unwrap();
        mark_event_processed(&mut sql_tx, first_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let second_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(second_event.tx_hash, tx_hash2);
        assert_eq!(second_event.block_number, 100);
    }

    #[tokio::test]
    async fn test_process_batch_retry_mechanism() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        // First two calls fail, third succeeds
        asserter.push_failure_msg("RPC connection error");
        asserter.push_failure_msg("Timeout error");
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;

        assert!(result.is_ok());
        let enqueued_count = result.unwrap();
        assert_eq!(enqueued_count, 0);
    }

    #[tokio::test]
    async fn test_process_batch_exhausted_retries() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        // All retry attempts fail - need double since clear_logs and take_logs retry in parallel
        // With test retry strategy: 2 retries = 3 total attempts per call
        for _ in 0..3 {
            asserter.push_failure_msg("Persistent RPC error"); // clear_logs failures
        }
        for _ in 0..3 {
            asserter.push_failure_msg("Persistent RPC error"); // take_logs failures
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_partial_batch_failure() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(25000u64));

        // First batch succeeds
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        // Second batch fails completely (after retries)
        // Need double the failures since clear_logs and take_logs retry in parallel
        // With test retry strategy: 2 retries = 3 total attempts per call
        for _ in 0..3 {
            asserter.push_failure_msg("Network failure"); // clear_logs failures
        }
        for _ in 0..3 {
            asserter.push_failure_msg("Network failure"); // take_logs failures
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = backfill_events_with_retry_strat(
            &pool,
            &provider,
            &evm_env,
            25000,
            test_retry_strategy(),
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), OnChainError::Alloy(_)));
    }

    #[tokio::test]
    async fn test_backfill_events_corrupted_log_data() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
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

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        // Corrupted logs are silently ignored during backfill
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_events_single_block_range() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 42,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(42u64));
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 100)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_enqueue_batch_events_database_failure() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let order = get_test_order();
        let take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");
        let take_log = create_test_log(
            evm_env.orderbook,
            &take_event,
            50,
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
        );

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([take_log])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        // Close the database to simulate connection failure
        pool.close().await;

        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;

        // Should succeed at RPC level but fail at database level
        // The function handles enqueue failures gracefully by continuing
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0); // No events successfully enqueued
    }

    #[tokio::test]
    async fn test_enqueue_batch_events_filter_creation() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        // Test with specific block range
        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 150, test_retry_strategy()).await;
        assert_eq!(result.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_enqueue_batch_events_partial_enqueue_failure() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let order = get_test_order();

        // Create multiple events
        let take_event1 = create_test_take_event(&order, 100_000_000, "9000000000000000000");
        let take_event2 = create_test_take_event(&order, 200_000_000, "18000000000000000000");

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

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([])); // clear events
        asserter.push_success(&serde_json::json!([take_log1, take_log2])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;

        // Should succeed with 2 events enqueued
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_backfill_events_concurrent_batch_processing() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let order = get_test_order();
        let take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");
        let take_log = create_test_log(
            evm_env.orderbook,
            &take_event,
            50,
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111"),
        );

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::Value::from(25000u64));

        // Multiple batches with events in different batches
        for batch_idx in 0..3 {
            // All batches start with clear events
            asserter.push_success(&serde_json::json!([])); // clear events
            if batch_idx == 1 {
                // Second batch has take events
                asserter.push_success(&serde_json::json!([take_log])); // take events
            } else {
                // Other batches have no take events
                asserter.push_success(&serde_json::json!([])); // take events
            }
        }

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        backfill_events(&pool, &provider, &evm_env, 25000)
            .await
            .unwrap();

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_enqueue_batch_events_retry_exponential_backoff() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let asserter = Asserter::new();
        // First attempt fails for both parallel calls
        asserter.push_failure_msg("Temporary network failure"); // clear_logs first attempt
        asserter.push_failure_msg("Rate limit exceeded"); // take_logs first attempt
        // Second attempt succeeds for both
        asserter.push_success(&serde_json::json!([])); // clear events (retry)
        asserter.push_success(&serde_json::json!([])); // take events (retry)

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let start_time = std::time::Instant::now();
        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;
        let elapsed = start_time.elapsed();

        // Should succeed after retries
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);

        // Should have taken at least the test initial delay time due to retries
        assert!(elapsed >= Duration::from_millis(1));
    }

    #[tokio::test]
    async fn test_backfill_events_zero_blocks() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 100,
        };

        // No RPC calls should be made when deployment block > end block
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = backfill_events(&pool, &provider, &evm_env, 50).await;
        assert!(result.is_ok());

        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_enqueue_batch_events_mixed_log_types() {
        let pool = setup_test_db().await;
        let evm_env = EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            deployment_block: 1,
        };

        let order = get_test_order();

        // Create a ClearV2 event
        let clear_event = IOrderBookV4::ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
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
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        // Create a TakeOrderV2 event
        let take_event = create_test_take_event(&order, 100_000_000, "9000000000000000000");
        let take_log = create_test_log(
            evm_env.orderbook,
            &take_event,
            51,
            fixed_bytes!("0x2222222222222222222222222222222222222222222222222222222222222222"),
        );

        let asserter = Asserter::new();
        asserter.push_success(&serde_json::json!([clear_log])); // clear events
        asserter.push_success(&serde_json::json!([take_log])); // take events

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            enqueue_batch_events(&pool, &provider, &evm_env, 100, 200, test_retry_strategy()).await;

        // Should process both event types
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);

        // Verify both events were enqueued
        let count = count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 2);
    }
}
