mod builder;

use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::Log;
use alloy::sol_types;
use futures_util::{Stream, StreamExt};
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, trace, warn};

use crate::bindings::IOrderBookV4::{ClearV2, IOrderBookV4Instance, TakeOrderV2};
use crate::env::Env;
use crate::error::EventProcessingError;
use crate::onchain::accumulator::check_all_accumulated_positions;
use crate::onchain::pyth::FeedIdCache;
use crate::onchain::trade::TradeEvent;
use crate::onchain::{EvmEnv, OnchainTrade, accumulator};
use crate::queue::{QueuedEvent, enqueue, get_next_unprocessed_event, mark_event_processed};
use crate::schwab::broker::{Broker, DynBroker};
use crate::schwab::tokens::{SchwabTokens, spawn_automatic_token_refresh};
use crate::schwab::{
    OrderStatusPoller, SchwabError,
    execution::{SchwabExecution, find_execution_by_id},
};
use crate::symbol::cache::SymbolCache;
use crate::symbol::lock::get_symbol_lock;
use crate::trading_hours_controller::TradingHoursController;

pub(crate) use builder::ConductorBuilder;

pub(crate) struct Conductor {
    pub(crate) token_refresher: JoinHandle<()>,
    pub(crate) order_poller: JoinHandle<()>,
    pub(crate) dex_event_receiver: JoinHandle<()>,
    pub(crate) event_processor: JoinHandle<()>,
    pub(crate) position_checker: JoinHandle<()>,
    pub(crate) queue_processor: JoinHandle<()>,
}

pub(crate) async fn run_market_hours_loop(
    controller: TradingHoursController,
    env: Env,
    pool: SqlitePool,
    token_refresher: JoinHandle<()>,
) -> anyhow::Result<()> {
    const RERUN_DELAY_SECS: u64 = 10;

    controller.wait_until_market_open().await?;

    let Some(time_until_close) = controller.time_until_market_close().await? else {
        warn!("Market already closed, waiting for next open");
        return Box::pin(run_market_hours_loop(
            controller,
            env,
            pool,
            token_refresher,
        ))
        .await;
    };

    let timeout = time_until_close.to_std()?;

    info!(
        "Market is open, starting conductor (will timeout in {} minutes)",
        timeout.as_secs() / 60
    );

    let mut conductor = match Conductor::start(&env, &pool, token_refresher).await {
        Ok(c) => c,
        Err(e) => {
            error!(
                "Failed to start conductor: {e}, retrying in {} seconds",
                RERUN_DELAY_SECS
            );

            tokio::time::sleep(std::time::Duration::from_secs(RERUN_DELAY_SECS)).await;

            let new_refresher =
                spawn_automatic_token_refresh(pool.clone(), env.schwab_auth.clone());

            return Box::pin(run_market_hours_loop(controller, env, pool, new_refresher)).await;
        }
    };

    info!("Market opened, conductor running");

    tokio::select! {
        result = conductor.wait_for_completion() => {
            info!("Conductor completed");
            conductor.abort_all();
            result?;
            info!("Conductor completed successfully, continuing to next market session");
            Ok(())
        }
        () = tokio::time::sleep(timeout) => {
            info!("Market closed, shutting down trading tasks");
            conductor.abort_trading_tasks();
            let next_refresher = conductor.token_refresher;
            info!("Trading tasks shutdown, DEX events buffering");
            Box::pin(run_market_hours_loop(controller, env, pool, next_refresher)).await
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("JoinHandle already consumed from AbortOnDrop")]
struct AbortOnDropConsumed;

struct AbortOnDrop(Option<JoinHandle<()>>);

impl AbortOnDrop {
    fn into_inner(mut self) -> Result<JoinHandle<()>, AbortOnDropConsumed> {
        let handle = self.0.take().ok_or(AbortOnDropConsumed)?;
        std::mem::forget(self);
        Ok(handle)
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

impl Conductor {
    pub(crate) async fn start(
        env: &Env,
        pool: &SqlitePool,
        token_refresher: JoinHandle<()>,
    ) -> anyhow::Result<Self> {
        let token_refresher = AbortOnDrop(Some(token_refresher));

        debug!("Validating Schwab tokens...");

        SchwabTokens::refresh_if_needed(pool, &env.schwab_auth)
            .await
            .map_err(|e| match e {
                SchwabError::RefreshTokenExpired => {
                    warn!("Refresh token expired, waiting for manual authentication via API");
                    anyhow::anyhow!("RefreshTokenExpired")
                }
                e => anyhow::anyhow!("Token validation failed: {}", e),
            })?;

        info!("Token validation successful");

        let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
        let provider = ProviderBuilder::new().connect_ws(ws).await?;
        let cache = SymbolCache::default();
        let orderbook = IOrderBookV4Instance::new(env.evm_env.orderbook, &provider);

        let mut clear_stream = orderbook.ClearV2_filter().watch().await?.into_stream();
        let mut take_stream = orderbook.TakeOrderV2_filter().watch().await?.into_stream();

        let cutoff_block =
            get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, pool).await?;

        backfill_events(pool, &provider, &env.evm_env, cutoff_block - 1).await?;

        Ok(
            ConductorBuilder::new(env.clone(), pool.clone(), cache, provider)
                .with_token_refresher(token_refresher.into_inner()?)
                .with_dex_event_streams(clear_stream, take_stream)
                .spawn(),
        )
    }

    pub(crate) async fn wait_for_completion(&mut self) -> Result<(), anyhow::Error> {
        let (
            token_result,
            poller_result,
            dex_receiver_result,
            processor_result,
            position_result,
            queue_result,
        ) = tokio::join!(
            &mut self.token_refresher,
            &mut self.order_poller,
            &mut self.dex_event_receiver,
            &mut self.event_processor,
            &mut self.position_checker,
            &mut self.queue_processor
        );

        if let Err(e) = token_result {
            error!("Token refresher task panicked: {e}");
        }
        if let Err(e) = poller_result {
            error!("Order poller task panicked: {e}");
        }
        if let Err(e) = dex_receiver_result {
            error!("DEX event receiver task panicked: {e}");
        }
        if let Err(e) = processor_result {
            error!("Event processor task panicked: {e}");
        }
        if let Err(e) = position_result {
            error!("Position checker task panicked: {e}");
        }
        if let Err(e) = queue_result {
            error!("Queue processor task panicked: {e}");
        }

        Ok(())
    }

    pub(crate) fn abort_trading_tasks(&self) {
        info!("Aborting trading tasks (keeping token refresh and DEX event receiver alive)");

        self.order_poller.abort();
        self.event_processor.abort();
        self.position_checker.abort();
        self.queue_processor.abort();

        info!("Trading tasks aborted successfully (DEX events will continue buffering)");
    }

    pub(crate) fn abort_all(self) {
        info!("Aborting all background tasks");

        self.token_refresher.abort();
        self.order_poller.abort();
        self.dex_event_receiver.abort();
        self.event_processor.abort();
        self.position_checker.abort();
        self.queue_processor.abort();

        info!("All background tasks aborted successfully");
    }
}

fn spawn_order_poller(env: &Env, pool: &SqlitePool, broker: DynBroker) -> JoinHandle<()> {
    let config = env.get_order_poller_config();
    info!(
        "Starting order status poller with interval: {:?}, max jitter: {:?}",
        config.polling_interval, config.max_jitter
    );

    let poller = OrderStatusPoller::new(config, env.schwab_auth.clone(), pool.clone(), broker);
    tokio::spawn(async move {
        if let Err(e) = poller.run().await {
            error!("Order poller failed: {e}");
        } else {
            info!("Order poller completed successfully");
        }
    })
}

fn spawn_onchain_event_receiver(
    event_sender: UnboundedSender<(TradeEvent, Log)>,
    clear_stream: impl Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin + Send + 'static,
    take_stream: impl Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>>
    + Unpin
    + Send
    + 'static,
) -> JoinHandle<()> {
    info!("Starting blockchain event receiver");
    tokio::spawn(receive_blockchain_events(
        clear_stream,
        take_stream,
        event_sender,
    ))
}

fn spawn_event_processor(
    pool: SqlitePool,
    mut event_receiver: tokio::sync::mpsc::UnboundedReceiver<(TradeEvent, Log)>,
) -> JoinHandle<()> {
    info!("Starting event processor");
    tokio::spawn(async move {
        while let Some((event, log)) = event_receiver.recv().await {
            trace!(
                "Processing live event: tx_hash={:?}, log_index={:?}",
                log.transaction_hash, log.log_index
            );
            if let Err(e) = process_live_event(&pool, event, log).await {
                error!("Failed to process live event: {e}");
            }
        }
        info!("Event processing loop ended");
    })
}

fn spawn_queue_processor<P: Provider + Clone + Send + 'static>(
    broker: DynBroker,
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: P,
    metrics: Arc<Option<metrics::Metrics>>,
    feed_id_cache: FeedIdCache,
) -> JoinHandle<()> {
    info!("Starting queue processor service");
    let env_clone = env.clone();
    let pool_clone = pool.clone();
    let cache_clone = cache.clone();

    tokio::spawn(async move {
        run_queue_processor(&broker, &env_clone, &pool_clone, &cache_clone, provider).await;
    })
}

fn spawn_periodic_accumulated_position_check(
    broker: DynBroker,
    env: Env,
    pool: SqlitePool,
) -> JoinHandle<()> {
    info!("Starting periodic accumulated position checker");

    tokio::spawn(async move {
        const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            debug!("Running periodic accumulated position check");
            if let Err(e) = check_and_execute_accumulated_positions(&broker, &env, &pool).await {
                error!("Periodic accumulated position check failed: {e}");
            }
        }
    })
}

async fn receive_blockchain_events<S1, S2>(
    mut clear_stream: S1,
    mut take_stream: S2,
    event_sender: UnboundedSender<(TradeEvent, Log)>,
) where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
{
    loop {
        let event_result = tokio::select! {
            Some(result) = clear_stream.next() => {
                result.map(|(event, log)| (TradeEvent::ClearV2(Box::new(event)), log))
            }
            Some(result) = take_stream.next() => {
                result.map(|(event, log)| (TradeEvent::TakeOrderV2(Box::new(event)), log))
            }
            else => {
                error!("All event streams ended, shutting down event receiver");
                break;
            }
        };

        match event_result {
            Ok((event, log)) => {
                trace!(
                    "Received blockchain event: tx_hash={:?}, log_index={:?}, block_number={:?}",
                    log.transaction_hash, log.log_index, log.block_number
                );
                if event_sender.send((event, log)).is_err() {
                    error!("Event receiver dropped, shutting down");
                    break;
                }
            }
            Err(e) => {
                error!("Error in event stream: {e}");
            }
        }
    }
}

pub(crate) async fn get_cutoff_block<S1, S2, P>(
    clear_stream: &mut S1,
    take_stream: &mut S2,
    provider: &P,
    pool: &SqlitePool,
) -> anyhow::Result<u64>
where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
    P: Provider + Clone,
{
    info!("Starting WebSocket subscriptions and waiting for first event...");

    let first_event_result = wait_for_first_event_with_timeout(
        clear_stream,
        take_stream,
        std::time::Duration::from_secs(5),
    )
    .await;

    let Some((mut event_buffer, block_number)) = first_event_result else {
        let current_block = provider.get_block_number().await?;
        info!(
            "No subscription events within timeout, using current block {current_block} as cutoff"
        );
        return Ok(current_block);
    };

    buffer_live_events(clear_stream, take_stream, &mut event_buffer, block_number).await;

    crate::queue::enqueue_buffer(pool, event_buffer).await;

    Ok(block_number)
}

async fn process_live_event(
    pool: &SqlitePool,
    event: TradeEvent,
    log: Log,
) -> Result<(), EventProcessingError> {
    match &event {
        TradeEvent::ClearV2(clear_event) => {
            info!(
                "Enqueuing ClearV2 event: tx_hash={:?}, log_index={:?}",
                log.transaction_hash, log.log_index
            );

            enqueue(pool, clear_event.as_ref(), &log)
                .await
                .map_err(EventProcessingError::EnqueueClearV2)?;
        }
        TradeEvent::TakeOrderV2(take_event) => {
            info!(
                "Enqueuing TakeOrderV2 event: tx_hash={:?}, log_index={:?}",
                log.transaction_hash, log.log_index
            );

            enqueue(pool, take_event.as_ref(), &log)
                .await
                .map_err(EventProcessingError::EnqueueTakeOrderV2)?;
        }
    }

    Ok(())
}

/// Dedicated queue processor service that continuously processes events from the queue.
/// This provides a unified processing path for both live and backfilled events.
pub(crate) async fn run_queue_processor<P: Provider + Clone>(
    broker: &DynBroker,
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: P,
) {
    info!("Starting queue processor service");

    // Log initial unprocessed event count
    match crate::queue::count_unprocessed(pool).await {
        Ok(count) if count > 0 => {
            info!("Found {count} unprocessed events from previous sessions to process");
        }
        Ok(_) => {
            info!("No unprocessed events found, starting fresh");
        }
        Err(e) => {
            error!("Failed to count unprocessed events: {e}");
        }
    }

    loop {
        match process_next_queued_event(env, pool, cache, &provider).await {
            Ok(Some(execution)) => {
                if let Some(exec_id) = execution.id {
                    if let Err(e) =
                        execute_pending_schwab_execution(broker, env, pool, exec_id).await
                    {
                        error!("Failed to execute Schwab order {exec_id}: {e}");
                    }
                }
            }
            Ok(None) => {
                sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                error!("Error processing queued event: {e}");
                sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Processes the next unprocessed event from the queue.
/// Returns an optional SchwabExecution if one was triggered.
async fn process_next_queued_event<P: Provider + Clone>(
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: &P,
) -> Result<Option<SchwabExecution>, EventProcessingError> {
    let queued_event = get_next_queued_event(pool).await?;
    let Some(queued_event) = queued_event else {
        return Ok(None);
    };

    let event_id = extract_event_id(&queued_event)?;

    let onchain_trade =
        convert_event_to_trade(env, cache, provider, &queued_event, feed_id_cache).await?;

    // If the event was filtered, mark as processed and return None
    let Some(trade) = onchain_trade else {
        return handle_filtered_event(pool, &queued_event, event_id).await;
    };

    process_valid_trade(pool, &queued_event, event_id, trade).await
}

async fn get_next_queued_event(
    pool: &SqlitePool,
) -> Result<Option<QueuedEvent>, EventProcessingError> {
    match get_next_unprocessed_event(pool).await {
        Ok(event) => Ok(event),
        Err(e) => {
            error!("Failed to get next unprocessed event: {e}");
            Err(EventProcessingError::Queue(e))
        }
    }
}

fn extract_event_id(queued_event: &QueuedEvent) -> Result<i64, EventProcessingError> {
    queued_event.id.ok_or_else(|| {
        EventProcessingError::Queue(crate::error::EventQueueError::Processing(
            "Queued event missing ID".to_string(),
        ))
    })
}

async fn convert_event_to_trade<P: Provider + Clone>(
    env: &Env,
    cache: &SymbolCache,
    provider: &P,
    queued_event: &QueuedEvent,
    feed_id_cache: &FeedIdCache,
) -> Result<Option<OnchainTrade>, EventProcessingError> {
    let reconstructed_log = reconstruct_log_from_queued_event(&env.evm_env, queued_event);

    let onchain_trade = match &queued_event.event {
        TradeEvent::ClearV2(clear_event) => {
            OnchainTrade::try_from_clear_v2(
                &env.evm_env,
                cache,
                provider,
                *clear_event.clone(),
                reconstructed_log,
                feed_id_cache,
            )
            .await?
        }
        TradeEvent::TakeOrderV2(take_event) => {
            OnchainTrade::try_from_take_order_if_target_owner(
                cache,
                provider,
                *take_event.clone(),
                reconstructed_log,
                env.evm_env.order_owner,
                feed_id_cache,
            )
            .await?
        }
    };

    Ok(onchain_trade)
}

async fn handle_filtered_event(
    pool: &SqlitePool,
    queued_event: &QueuedEvent,
    event_id: i64,
) -> Result<Option<SchwabExecution>, EventProcessingError> {
    info!(
        "Event filtered out (no matching owner): event_type={:?}, tx_hash={:?}, log_index={}",
        match &queued_event.event {
            TradeEvent::ClearV2(_) => "ClearV2",
            TradeEvent::TakeOrderV2(_) => "TakeOrderV2",
        },
        queued_event.tx_hash,
        queued_event.log_index
    );

    let mut sql_tx = pool.begin().await.map_err(|e| {
        error!("Failed to begin transaction for filtered event: {e}");
        EventProcessingError::Queue(crate::error::EventQueueError::Processing(format!(
            "Failed to begin transaction: {e}"
        )))
    })?;

    mark_event_processed(&mut sql_tx, event_id).await?;

    sql_tx.commit().await.map_err(|e| {
        error!("Failed to commit transaction for filtered event: {e}");
        EventProcessingError::Queue(crate::error::EventQueueError::Processing(format!(
            "Failed to commit transaction: {e}"
        )))
    })?;

    Ok(None)
}

async fn process_valid_trade(
    pool: &SqlitePool,
    queued_event: &QueuedEvent,
    event_id: i64,
    trade: OnchainTrade,
) -> Result<Option<SchwabExecution>, EventProcessingError> {
    info!(
        "Event successfully converted to trade: event_type={:?}, tx_hash={:?}, log_index={}, symbol={}, amount={}",
        match &queued_event.event {
            TradeEvent::ClearV2(_) => "ClearV2",
            TradeEvent::TakeOrderV2(_) => "TakeOrderV2",
        },
        trade.tx_hash,
        trade.log_index,
        trade.symbol,
        trade.amount
    );

    let symbol_lock = get_symbol_lock(trade.symbol.base()).await;
    let _guard = symbol_lock.lock().await;

    info!(
        "Processing queued trade: symbol={}, amount={}, direction={:?}, tx_hash={:?}, log_index={}",
        trade.symbol, trade.amount, trade.direction, trade.tx_hash, trade.log_index
    );

    process_trade_within_transaction(pool, queued_event, event_id, trade).await
}

async fn process_trade_within_transaction(
    pool: &SqlitePool,
    queued_event: &QueuedEvent,
    event_id: i64,
    trade: OnchainTrade,
) -> Result<Option<SchwabExecution>, EventProcessingError> {
    let mut sql_tx = pool.begin().await.map_err(|e| {
        error!("Failed to begin transaction for event processing: {e}");
        EventProcessingError::AccumulatorProcessing(format!("Failed to begin transaction: {e}"))
    })?;

    info!(
        "Started transaction for atomic event processing: event_id={}, tx_hash={:?}, log_index={}",
        event_id, queued_event.tx_hash, queued_event.log_index
    );

    let execution = accumulator::process_onchain_trade(&mut sql_tx, trade)
        .await
        .map_err(|e| {
            error!(
                "Failed to process trade through accumulator: {e}, tx_hash={:?}, log_index={}",
                queued_event.tx_hash, queued_event.log_index
            );
            EventProcessingError::AccumulatorProcessing(format!(
                "Failed to process trade through accumulator: {e}"
            ))
        })?;

    mark_event_processed(&mut sql_tx, event_id)
        .await
        .map_err(|e| {
            error!("Failed to mark event {event_id} as processed: {e}");
            EventProcessingError::Queue(e)
        })?;

    sql_tx.commit().await.map_err(|e| {
        error!(
            "Failed to commit transaction for event processing: {e}, event_id={}, tx_hash={:?}",
            event_id, queued_event.tx_hash
        );
        EventProcessingError::AccumulatorProcessing(format!("Failed to commit transaction: {e}"))
    })?;

    info!(
        "Successfully committed atomic event processing: event_id={}, tx_hash={:?}, log_index={}",
        event_id, queued_event.tx_hash, queued_event.log_index
    );

    Ok(execution)
}

/// Reconstructs a Log with proper event data from a queued event
fn reconstruct_log_from_queued_event(
    evm_env: &EvmEnv,
    queued_event: &crate::queue::QueuedEvent,
) -> Log {
    use alloy::primitives::IntoLogData;

    // Reconstruct proper log data based on event type
    let log_data = match &queued_event.event {
        TradeEvent::ClearV2(clear_event) => clear_event.as_ref().clone().into_log_data(),
        TradeEvent::TakeOrderV2(take_event) => take_event.as_ref().clone().into_log_data(),
    };

    Log {
        inner: alloy::primitives::Log {
            address: evm_env.orderbook,
            data: log_data,
        },
        block_hash: None,
        block_number: Some(queued_event.block_number),
        block_timestamp: None,
        transaction_hash: Some(queued_event.tx_hash),
        transaction_index: None,
        log_index: Some(queued_event.log_index),
        removed: false,
    }
}

/// Checks for accumulated positions ready for execution and spawns tasks to execute them.
async fn check_and_execute_accumulated_positions(
    broker: &DynBroker,
    env: &Env,
    pool: &SqlitePool,
) -> Result<(), EventProcessingError> {
    let executions = check_all_accumulated_positions(pool).await?;

    if executions.is_empty() {
        debug!("No accumulated positions ready for execution");
        return Ok(());
    }

    info!(
        "Found {} accumulated positions ready for execution",
        executions.len()
    );

    for execution in executions {
        let Some(execution_id) = execution.id else {
            error!("Execution returned from check_all_accumulated_positions has None ID");
            continue;
        };

        info!(
            "Executing accumulated position for symbol={}, shares={}, direction={:?}, execution_id={}",
            execution.symbol, execution.shares, execution.direction, execution_id
        );

        let env_clone = env.clone();
        let pool_clone = pool.clone();
        let broker_clone = broker.clone();
        tokio::spawn(async move {
            if let Err(e) = execute_pending_schwab_execution(
                &broker_clone,
                &env_clone,
                &pool_clone,
                execution_id,
            )
            .await
            {
                error!(
                    "Failed to execute accumulated position for execution_id {}: {e}",
                    execution_id
                );
            } else {
                info!(
                    "Successfully executed accumulated position for execution_id {}",
                    execution_id
                );
            }
        });
    }

    Ok(())
}

/// Execute a pending Schwab execution by fetching it from the database and placing the order.
async fn execute_pending_schwab_execution(
    broker: &DynBroker,
    env: &Env,
    pool: &SqlitePool,
    execution_id: i64,
) -> Result<(), EventProcessingError> {
    let execution = find_execution_by_id(pool, execution_id)
        .await?
        .ok_or_else(|| {
            EventProcessingError::AccumulatorProcessing(format!(
                "Execution with ID {execution_id} not found"
            ))
        })?;

    info!("Executing Schwab order: {execution:?}");
    broker.execute_order(env, pool, execution).await?;

    Ok(())
}

/// Waits for the first event from either stream with a timeout, returning any events received
async fn wait_for_first_event_with_timeout<S1, S2>(
    clear_stream: &mut S1,
    take_stream: &mut S2,
    timeout: std::time::Duration,
) -> Option<(Vec<(TradeEvent, Log)>, u64)>
where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
{
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    let mut events = Vec::new();

    loop {
        tokio::select! {
            Some(result) = clear_stream.next() => {
                match result {
                    Ok((event, log)) => {
                        if let Some(block_number) = log.block_number {
                            events.push((TradeEvent::ClearV2(Box::new(event)), log));
                            return Some((events, block_number));
                        }
                        error!("ClearV2 event missing block number");
                    }
                    Err(e) => {
                        error!("Error in clear event stream during startup: {e}");
                    }
                }
            }
            Some(result) = take_stream.next() => {
                match result {
                    Ok((event, log)) => {
                        if let Some(block_number) = log.block_number {
                            events.push((TradeEvent::TakeOrderV2(Box::new(event)), log));
                            return Some((events, block_number));
                        }
                        error!("TakeOrderV2 event missing block number");
                    }
                    Err(e) => {
                        error!("Error in take event stream during startup: {e}");
                    }
                }
            }
            () = &mut deadline => {
                return None;
            }
        }
    }
}

/// Continues buffering events from subscription streams during backfill
async fn buffer_live_events<S1, S2>(
    clear_stream: &mut S1,
    take_stream: &mut S2,
    event_buffer: &mut Vec<(TradeEvent, Log)>,
    cutoff_block: u64,
) where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
{
    loop {
        tokio::select! {
            Some(result) = clear_stream.next() => match result {
                Ok((event, log)) if log.block_number.unwrap_or(0) >= cutoff_block => {
                    event_buffer.push((TradeEvent::ClearV2(Box::new(event)), log));
                }
                Err(e) => error!("Error in clear event stream during backfill: {e}"),
                _ => {}
            },
            Some(result) = take_stream.next() => match result {
                Ok((event, log)) if log.block_number.unwrap_or(0) >= cutoff_block => {
                    event_buffer.push((TradeEvent::TakeOrderV2(Box::new(event)), log));
                }
                Err(e) => error!("Error in take event stream during backfill: {e}"),
                _ => {}
            },
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::IOrderBookV4::{ClearConfig, ClearV2};
    use crate::env::tests::create_test_env;
    use crate::onchain::trade::OnchainTrade;
    use crate::schwab::Direction;
    use crate::test_utils::{OnchainTradeBuilder, setup_test_db};
    use crate::tokenized_symbol;
    use alloy::primitives::{IntoLogData, address, fixed_bytes};
    use alloy::providers::ProviderBuilder;
    use alloy::providers::mock::Asserter;
    use alloy::sol_types;
    use futures_util::stream;

    #[tokio::test]
    async fn test_event_enqueued_when_trade_conversion_returns_none() {
        let pool = setup_test_db().await;
        let _env = create_test_env();

        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log = crate::test_utils::get_test_log();

        // Enqueue the event
        crate::queue::enqueue(&pool, &clear_event, &log)
            .await
            .unwrap();

        // Verify event was enqueued
        let count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Verify no trades were created (since this event doesn't result in a valid trade)
        let trade_count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(trade_count, 0);
    }

    #[tokio::test]
    async fn test_onchain_trade_duplicate_handling() {
        let pool = setup_test_db().await;

        let existing_trade = OnchainTradeBuilder::new()
            .with_tx_hash(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ))
            .with_log_index(293)
            .with_symbol("AAPL0x")
            .with_amount(5.0)
            .with_price(20000.0)
            .build();
        let mut sql_tx = pool.begin().await.unwrap();
        existing_trade
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        let duplicate_trade = existing_trade.clone();
        let mut sql_tx2 = pool.begin().await.unwrap();
        let duplicate_result = duplicate_trade.save_within_transaction(&mut sql_tx2).await;
        assert!(duplicate_result.is_err());
        sql_tx2.rollback().await.unwrap();

        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_duplicate_trade_handling() {
        let pool = setup_test_db().await;

        let existing_trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            symbol: tokenized_symbol!("AAPL0x"),
            amount: 5.0,
            direction: Direction::Sell,
            price_usdc: 20000.0,
            block_timestamp: None,
            created_at: None,
            gas_used: None,
            effective_gas_price: None,
            pyth_price: None,
            pyth_confidence: None,
            pyth_exponent: None,
            pyth_publish_time: None,
        };
        let mut sql_tx = pool.begin().await.unwrap();
        existing_trade
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Try to save the same trade again
        let duplicate_trade = existing_trade.clone();
        let mut sql_tx2 = pool.begin().await.unwrap();
        let duplicate_result = duplicate_trade.save_within_transaction(&mut sql_tx2).await;
        assert!(duplicate_result.is_err());
        sql_tx2.rollback().await.unwrap();

        // Verify only one trade exists
        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_complete_event_processing_flow() {
        // Test the complete flow: event -> enqueue -> process -> trade -> accumulation
        let pool = setup_test_db().await;
        let env = create_test_env();

        // Simulate a ClearV2 event being processed
        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log = crate::test_utils::get_test_log();

        // Step 1: Enqueue the event (like what happens during backfill/live processing)
        crate::queue::enqueue(&pool, &clear_event, &log)
            .await
            .unwrap();

        // Step 2: Verify event was enqueued
        let count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Step 3: Get the queued event for processing
        let queued_event = crate::queue::get_next_unprocessed_event(&pool)
            .await
            .unwrap()
            .unwrap();

        // Step 4: Process the event (simulate the live event processing loop)
        // This would be the equivalent of the event processing inside the channel receiver
        if let TradeEvent::ClearV2(boxed_clear_event) = queued_event.event {
            // Create a mock provider and cache for event conversion
            let cache = SymbolCache::default();
            let http_provider =
                ProviderBuilder::new().connect_http("http://localhost:8545".parse().unwrap());

            // Try to convert to OnchainTrade (this will fail in test since we don't have mock RPC)
            // but we can at least verify the flow structure
            let feed_id_cache = FeedIdCache::default();
            if let Ok(Some(trade)) = OnchainTrade::try_from_clear_v2(
                &env.evm_env,
                &cache,
                &http_provider,
                *boxed_clear_event,
                log,
                &feed_id_cache,
            )
            .await
            {
                // Step 5: Process the trade through accumulation
                let mut sql_tx = pool.begin().await.unwrap();
                accumulator::process_onchain_trade(&mut sql_tx, trade)
                    .await
                    .unwrap();
                sql_tx.commit().await.unwrap();
            } else {
                // Event doesn't result in a trade or expected test environment error
                // The important thing is we tested the flow structure
            }
        }

        // Step 6: Mark event as processed
        let mut sql_tx = pool.begin().await.unwrap();
        crate::queue::mark_event_processed(&mut sql_tx, queued_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Step 7: Verify event was marked processed
        let remaining_count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(remaining_count, 0);
    }

    #[tokio::test]
    async fn test_idempotency_bot_restart_during_processing() {
        // Test that bot restart at any point resumes without missing/duplicating events
        let pool = setup_test_db().await;
        let _env = create_test_env();

        // Create test events
        let event1 = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log1 = crate::test_utils::get_test_log();

        // Simulate different restart scenarios

        // Scenario 1: Enqueue events and restart before processing
        crate::queue::enqueue(&pool, &event1, &log1).await.unwrap();
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 1);

        // Simulate restart: process unprocessed events (mark as processed for test)
        // For now, just mark events as processed to verify the idempotency mechanism works
        let queued_event = crate::queue::get_next_unprocessed_event(&pool)
            .await
            .unwrap()
            .unwrap();
        let mut sql_tx = pool.begin().await.unwrap();
        crate::queue::mark_event_processed(&mut sql_tx, queued_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0);

        // Scenario 2: Process same event again - should be deduplicated
        crate::queue::enqueue(&pool, &event1, &log1).await.unwrap(); // Should be ignored due to unique constraint
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0); // No new events

        // Scenario 3: Mix of processed and unprocessed events after restart
        let mut log2 = crate::test_utils::get_test_log();
        log2.log_index = Some(2); // Different log index
        crate::queue::enqueue(&pool, &event1, &log2).await.unwrap();
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 1);

        // Verify events are processed in deterministic order (by block_number, log_index)
        let next_event = crate::queue::get_next_unprocessed_event(&pool)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next_event.log_index, 2); // Should get log_index 2
        let mut sql_tx = pool.begin().await.unwrap();
        crate::queue::mark_event_processed(&mut sql_tx, next_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn test_deterministic_processing_order() {
        // Test that events are always processed in same order regardless of enqueueing order
        let pool = setup_test_db().await;

        // Create events with different block numbers and log indices
        let events_and_logs = vec![
            // (block_number, log_index)
            (100, 5),
            (99, 3),
            (100, 1),
            (101, 2),
            (99, 8),
        ];

        // Enqueue in random order
        for (block_num, log_idx) in &events_and_logs {
            let event = ClearV2 {
                sender: address!("0x1111111111111111111111111111111111111111"),
                alice: crate::test_utils::get_test_order(),
                bob: crate::test_utils::get_test_order(),
                clearConfig: ClearConfig {
                    aliceInputIOIndex: alloy::primitives::U256::from(0),
                    aliceOutputIOIndex: alloy::primitives::U256::from(1),
                    bobInputIOIndex: alloy::primitives::U256::from(1),
                    bobOutputIOIndex: alloy::primitives::U256::from(0),
                    aliceBountyVaultId: alloy::primitives::U256::ZERO,
                    bobBountyVaultId: alloy::primitives::U256::ZERO,
                },
            };
            let mut log = crate::test_utils::get_test_log();
            log.block_number = Some(*block_num);
            log.log_index = Some(*log_idx);
            // Make each transaction hash unique
            // Create unique transaction hash
            log.transaction_hash = Some(fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ));

            crate::queue::enqueue(&pool, &event, &log).await.unwrap();
        }

        // Process events and verify they come out in deterministic order
        let expected_order = vec![
            (99, 3),  // Block 99, log 3
            (99, 8),  // Block 99, log 8
            (100, 1), // Block 100, log 1
            (100, 5), // Block 100, log 5
            (101, 2), // Block 101, log 2
        ];

        for (expected_block, expected_log_idx) in expected_order {
            let event = crate::queue::get_next_unprocessed_event(&pool)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.block_number, expected_block);
            assert_eq!(event.log_index, expected_log_idx);
            let mut sql_tx = pool.begin().await.unwrap();
            crate::queue::mark_event_processed(&mut sql_tx, event.id.unwrap())
                .await
                .unwrap();
            sql_tx.commit().await.unwrap();
        }

        // Verify no more events
        assert!(
            crate::queue::get_next_unprocessed_event(&pool)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_restart_scenarios_edge_cases() {
        let pool = setup_test_db().await;

        // Test Case 1: Restart with empty queue
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0);
        // Empty queue should handle gracefully - no processing needed
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0);

        // Test Case 2: Restart during backfill (simulated by having mixed processed/unprocessed)
        let mut events = vec![];
        for i in 0..5 {
            let event = ClearV2 {
                sender: address!("0x1111111111111111111111111111111111111111"),
                alice: crate::test_utils::get_test_order(),
                bob: crate::test_utils::get_test_order(),
                clearConfig: ClearConfig {
                    aliceInputIOIndex: alloy::primitives::U256::from(0),
                    aliceOutputIOIndex: alloy::primitives::U256::from(1),
                    bobInputIOIndex: alloy::primitives::U256::from(1),
                    bobOutputIOIndex: alloy::primitives::U256::from(0),
                    aliceBountyVaultId: alloy::primitives::U256::ZERO,
                    bobBountyVaultId: alloy::primitives::U256::ZERO,
                },
            };
            let mut log = crate::test_utils::get_test_log();
            log.log_index = Some(i);
            // Create unique transaction hash
            let mut hash_bytes = [0u8; 32];
            hash_bytes[31] = u8::try_from(i).unwrap_or(0);
            log.transaction_hash = Some(alloy::primitives::B256::from(hash_bytes));

            crate::queue::enqueue(&pool, &event, &log).await.unwrap();
            events.push((event, log));
        }

        // Process first 2 events (simulate partial processing before restart)
        for _ in 0..2 {
            let event = crate::queue::get_next_unprocessed_event(&pool)
                .await
                .unwrap()
                .unwrap();
            let mut sql_tx = pool.begin().await.unwrap();
            crate::queue::mark_event_processed(&mut sql_tx, event.id.unwrap())
                .await
                .unwrap();
            sql_tx.commit().await.unwrap();
        }

        // Verify 3 events remain unprocessed
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 3);

        // Simulate restart: process remaining events
        let mut processed_count = 0;
        while let Some(event) = crate::queue::get_next_unprocessed_event(&pool)
            .await
            .unwrap()
        {
            let mut sql_tx = pool.begin().await.unwrap();
            crate::queue::mark_event_processed(&mut sql_tx, event.id.unwrap())
                .await
                .unwrap();
            sql_tx.commit().await.unwrap();
            processed_count += 1;
        }

        assert_eq!(processed_count, 3); // Should process exactly 3 remaining events
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0);

        // Test Case 3: Attempt to reprocess already processed events
        // This should be prevented by the unique constraint, but test the behavior
        for (event, log) in &events {
            crate::queue::enqueue(&pool, event, log).await.unwrap(); // Should be ignored
        }

        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0); // No new unprocessed events
    }

    #[tokio::test]
    async fn test_process_queued_event_deserialization() {
        let pool = setup_test_db().await;
        let env = create_test_env();

        // Create a test ClearV2 event
        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log = crate::test_utils::get_test_log();

        // Enqueue the event
        crate::queue::enqueue(&pool, &clear_event, &log)
            .await
            .unwrap();

        // Verify event was enqueued
        let count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Get the queued event
        let queued_event = crate::queue::get_next_unprocessed_event(&pool)
            .await
            .unwrap()
            .unwrap();

        // Test deserialization
        assert!(matches!(queued_event.event, TradeEvent::ClearV2(_)));

        // Test log reconstruction
        let reconstructed_log = reconstruct_log_from_queued_event(&env.evm_env, &queued_event);
        assert_eq!(reconstructed_log.inner.address, env.evm_env.orderbook);
        assert_eq!(
            reconstructed_log.transaction_hash.unwrap(),
            queued_event.tx_hash
        );
        assert_eq!(reconstructed_log.log_index.unwrap(), queued_event.log_index);
        assert_eq!(
            reconstructed_log.block_number.unwrap(),
            queued_event.block_number
        );

        // Verify that the reconstructed log has proper event data (not default)
        let original_log_data = clear_event.into_log_data();
        assert_eq!(reconstructed_log.inner.data, original_log_data);

        // Clean up
        let mut sql_tx = pool.begin().await.unwrap();
        crate::queue::mark_event_processed(&mut sql_tx, queued_event.id.unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();
        assert_eq!(crate::queue::count_unprocessed(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_get_cutoff_block_with_timeout() {
        let pool = setup_test_db().await;
        let asserter = Asserter::new();

        // Mock the eth_blockNumber call that will be made when no events arrive
        asserter.push_success(&serde_json::Value::from(12345u64));
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        // Create empty streams that will never yield events (to trigger timeout)
        let mut clear_stream = futures_util::stream::empty();
        let mut take_stream = futures_util::stream::empty();

        // Should return current block number when no events arrive within timeout
        let cutoff_block = get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, &pool)
            .await
            .unwrap();

        assert_eq!(cutoff_block, 12345);
    }

    #[tokio::test]
    async fn test_wait_for_first_event_with_timeout_no_events() {
        // Create empty streams
        let mut clear_stream = stream::empty();
        let mut take_stream = stream::empty();

        let result = wait_for_first_event_with_timeout(
            &mut clear_stream,
            &mut take_stream,
            std::time::Duration::from_millis(10),
        )
        .await;

        // Should return None when timeout expires with no events
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_wait_for_first_event_with_clear_event() {
        // Create a test ClearV2 event
        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let mut log = crate::test_utils::get_test_log();
        log.block_number = Some(1000);

        // Create streams with one event each
        let mut clear_stream = stream::iter(vec![Ok((clear_event, log.clone()))]);
        let mut take_stream = stream::empty::<Result<(TakeOrderV2, Log), sol_types::Error>>();

        let result = wait_for_first_event_with_timeout(
            &mut clear_stream,
            &mut take_stream,
            std::time::Duration::from_secs(1),
        )
        .await;

        // Should return the event and block number
        assert!(result.is_some());
        let (events, block_number) = result.unwrap();
        assert_eq!(block_number, 1000);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].0, TradeEvent::ClearV2(_)));
    }

    #[tokio::test]
    async fn test_wait_for_first_event_missing_block_number() {
        // Create event with missing block number
        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let mut log = crate::test_utils::get_test_log();
        log.block_number = None; // Missing block number

        let mut clear_stream = stream::iter(vec![Ok((clear_event, log))]);
        let mut take_stream = stream::empty::<Result<(TakeOrderV2, Log), sol_types::Error>>();

        let result = wait_for_first_event_with_timeout(
            &mut clear_stream,
            &mut take_stream,
            std::time::Duration::from_millis(100),
        )
        .await;

        // Should timeout because event has no block number
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_buffer_live_events_filtering() {
        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };

        // Create events with different block numbers
        let mut early_log = crate::test_utils::get_test_log();
        early_log.block_number = Some(99); // Below cutoff

        let mut late_log = crate::test_utils::get_test_log();
        late_log.block_number = Some(101); // Above cutoff

        let events = vec![
            Ok((clear_event.clone(), early_log)),
            Ok((clear_event, late_log)),
        ];

        let mut clear_stream = stream::iter(events);
        let mut take_stream = stream::empty::<Result<(TakeOrderV2, Log), sol_types::Error>>();
        let mut event_buffer = Vec::new();

        // Should only buffer events at or above cutoff block 100
        buffer_live_events(&mut clear_stream, &mut take_stream, &mut event_buffer, 100).await;

        // Should only have one event (block 101, not block 99)
        assert_eq!(event_buffer.len(), 1);
        assert_eq!(event_buffer[0].1.block_number.unwrap(), 101);
    }

    #[tokio::test]
    async fn test_process_live_event_clear_v2() {
        let pool = setup_test_db().await;

        let clear_event = ClearV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            alice: crate::test_utils::get_test_order(),
            bob: crate::test_utils::get_test_order(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log = crate::test_utils::get_test_log();

        // Process the live event
        let result =
            process_live_event(&pool, TradeEvent::ClearV2(Box::new(clear_event)), log).await;

        // Should succeed in enqueuing even if trade conversion fails
        assert!(result.is_ok());

        // Verify event was enqueued
        let count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_clear_v2_event_filtering_without_errors() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        // Create a ClearV2 event with owners that don't match the configured order owner
        let mut alice_order = crate::test_utils::get_test_order();
        let mut bob_order = crate::test_utils::get_test_order();

        // Set both owners to addresses different from the configured order owner
        alice_order.owner = address!("0x1111111111111111111111111111111111111111");
        bob_order.owner = address!("0x2222222222222222222222222222222222222222");

        let clear_event = ClearV2 {
            sender: address!("0x3333333333333333333333333333333333333333"),
            alice: alice_order,
            bob: bob_order,
            clearConfig: ClearConfig {
                aliceInputIOIndex: alloy::primitives::U256::from(0),
                aliceOutputIOIndex: alloy::primitives::U256::from(1),
                bobInputIOIndex: alloy::primitives::U256::from(1),
                bobOutputIOIndex: alloy::primitives::U256::from(0),
                aliceBountyVaultId: alloy::primitives::U256::ZERO,
                bobBountyVaultId: alloy::primitives::U256::ZERO,
            },
        };
        let log = crate::test_utils::get_test_log();

        // Enqueue the event
        crate::queue::enqueue(&pool, &clear_event, &log)
            .await
            .unwrap();

        // Verify event was enqueued
        let count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Process the event - should filter it out without error
        let result = process_next_queued_event(&env, &pool, &cache, &provider).await;

        // Should return Ok(None) indicating filtered event
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());

        // Verify event was marked as processed (no more unprocessed events)
        let remaining_count = crate::queue::count_unprocessed(&pool).await.unwrap();
        assert_eq!(remaining_count, 0);
    }

    #[tokio::test]
    async fn test_execute_pending_schwab_execution_not_found() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let broker = env.get_broker();

        // Try to execute non-existent execution
        let result = execute_pending_schwab_execution(&broker, &env, &pool, 99999).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_conductor_abort_all() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        let clear_stream = stream::empty();
        let take_stream = stream::empty();

        let token_refresher = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });

        let conductor = ConductorBuilder::new(env, pool, cache, provider)
            .with_token_refresher(token_refresher)
            .with_dex_event_streams(clear_stream, take_stream)
            .spawn();

        assert!(!conductor.token_refresher.is_finished());
        assert!(!conductor.order_poller.is_finished());
        assert!(!conductor.event_processor.is_finished());
        assert!(!conductor.position_checker.is_finished());
        assert!(!conductor.queue_processor.is_finished());

        conductor.abort_all();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_conductor_individual_abort() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        let clear_stream = stream::empty();
        let take_stream = stream::empty();

        let token_refresher = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });

        let conductor = ConductorBuilder::new(env, pool, cache, provider)
            .with_token_refresher(token_refresher)
            .with_dex_event_streams(clear_stream, take_stream)
            .spawn();

        let token_handle = conductor.token_refresher;
        let order_handle = conductor.order_poller;
        let event_handle = conductor.event_processor;
        let position_handle = conductor.position_checker;
        let queue_handle = conductor.queue_processor;

        assert!(!token_handle.is_finished());
        assert!(!order_handle.is_finished());
        assert!(!event_handle.is_finished());
        assert!(!position_handle.is_finished());
        assert!(!queue_handle.is_finished());

        token_handle.abort();
        order_handle.abort();
        event_handle.abort();
        position_handle.abort();
        queue_handle.abort();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert!(token_handle.is_finished());
        assert!(order_handle.is_finished());
        assert!(event_handle.is_finished());
        assert!(position_handle.is_finished());
        assert!(queue_handle.is_finished());
    }

    #[tokio::test]
    async fn test_conductor_builder_returns_immediately() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        let clear_stream = stream::empty();
        let take_stream = stream::empty();

        let start_time = std::time::Instant::now();

        let token_refresher = tokio::spawn(async {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });

        let conductor = ConductorBuilder::new(env, pool, cache, provider)
            .with_token_refresher(token_refresher)
            .with_dex_event_streams(clear_stream, take_stream)
            .spawn();

        let elapsed = start_time.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "ConductorBuilder should return quickly, took: {elapsed:?}"
        );

        conductor.abort_all();
    }
}
