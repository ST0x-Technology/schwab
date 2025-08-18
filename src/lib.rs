use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::Log;
use alloy::sol_types;
use clap::Parser;
use futures_util::{Stream, StreamExt};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tracing::{Level, error, info};

mod bindings;
pub mod cli;
pub mod error;
mod lock;
pub mod onchain;
pub mod queue;
pub mod schwab;
mod symbol_cache;

#[cfg(test)]
pub mod test_utils;

use bindings::IOrderBookV4::{ClearV2, IOrderBookV4Instance, TakeOrderV2};
use onchain::{EvmEnv, OnchainTrade, accumulator};
use queue::{enqueue_blockchain_event, get_next_unprocessed_event, mark_event_processed};
use schwab::{SchwabAuthEnv, execution::find_execution_by_id, order::execute_schwab_order};
use symbol_cache::SymbolCache;

/// Global symbol-level locks to prevent race conditions during concurrent trade processing.
/// Each symbol gets its own mutex to ensure atomic accumulation operations.
static SYMBOL_LOCKS: LazyLock<RwLock<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Acquires a symbol-specific lock to ensure atomic trade processing.
/// Creates a new lock for the symbol if one doesn't exist.
async fn get_symbol_lock(symbol: &str) -> Arc<Mutex<()>> {
    // First try to get existing lock with read lock (most common case)
    {
        let locks_read = SYMBOL_LOCKS.read().await;
        if let Some(lock) = locks_read.get(symbol) {
            return lock.clone();
        }
    }

    // If lock doesn't exist, acquire write lock and create new one
    let mut locks_write = SYMBOL_LOCKS.write().await;
    locks_write
        .entry(symbol.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(clap::ValueEnum, Debug, Clone)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for Level {
    fn from(log_level: LogLevel) -> Self {
        match log_level {
            LogLevel::Trace => Self::TRACE,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Info => Self::INFO,
            LogLevel::Warn => Self::WARN,
            LogLevel::Error => Self::ERROR,
        }
    }
}

impl From<&LogLevel> for Level {
    fn from(log_level: &LogLevel) -> Self {
        match log_level {
            LogLevel::Trace => Self::TRACE,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Info => Self::INFO,
            LogLevel::Warn => Self::WARN,
            LogLevel::Error => Self::ERROR,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct Env {
    #[clap(long = "db", env)]
    pub database_url: String,
    #[clap(long, env, default_value = "debug")]
    pub log_level: LogLevel,
    #[clap(flatten)]
    pub schwab_auth: SchwabAuthEnv,
    #[clap(flatten)]
    pub evm_env: EvmEnv,
}

impl Env {
    pub async fn get_sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        SqlitePool::connect(&self.database_url).await
    }
}

pub fn setup_tracing(log_level: &LogLevel) {
    let level: Level = log_level.into();
    let default_filter = format!("rain_schwab={level},auth={level},main={level}");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
}

pub async fn run(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    info!("Validating Schwab tokens...");
    schwab::tokens::SchwabTokens::refresh_if_needed(&pool, &env.schwab_auth).await?;
    info!("Token validation successful");

    let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let cache = SymbolCache::default();
    let orderbook = IOrderBookV4Instance::new(env.evm_env.orderbook, &provider);

    schwab::tokens::SchwabTokens::spawn_automatic_token_refresh(
        pool.clone(),
        env.schwab_auth.clone(),
    );

    // Process any unprocessed events from previous runs
    process_unprocessed_events(&pool).await?;

    let clear_filter = orderbook.ClearV2_filter().watch().await?;
    let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    let mut clear_stream = clear_filter.into_stream();
    let mut take_stream = take_filter.into_stream();

    loop {
        step(
            &mut clear_stream,
            &mut take_stream,
            &env,
            &pool,
            &cache,
            &provider,
        )
        .await?;
    }
}

async fn step<S1, S2, P>(
    clear_stream: &mut S1,
    take_stream: &mut S2,
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: &P,
) -> anyhow::Result<()>
where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
    P: Provider + Clone,
{
    let onchain_trade = tokio::select! {
        Some(next_res) = clear_stream.next() => {
            let (event, log) = next_res?;

            if let Err(e) = enqueue(pool, &event, &log).await {
                error!("Failed to enqueue ClearV2 event: {e}");
            }

            OnchainTrade::try_from_clear_v2(&env.evm_env, cache, provider, event, log).await?
        }
        Some(take) = take_stream.next() => {
            let (event, log) = take?;

            if let Err(e) = enqueue(pool, &event, &log).await {
                error!("Failed to enqueue TakeOrderV2 event: {e}");
            }

            OnchainTrade::try_from_take_order_if_target_order(cache, provider, event, log, env.evm_env.order_hash).await?
        }
    };

    let Some(onchain_trade) = onchain_trade else {
        return Ok(());
    };

    // Acquire symbol-level lock to prevent race conditions during accumulation
    let symbol_lock = get_symbol_lock(&onchain_trade.symbol).await;
    let _guard = symbol_lock.lock().await;

    // Save values for logging before the trade is moved
    let tx_hash = onchain_trade.tx_hash;
    let log_index = onchain_trade.log_index;

    let execution = accumulator::add_trade(pool, onchain_trade).await?;
    let execution_id = execution.and_then(|exec| exec.id);

    if let Some(exec_id) = execution_id {
        info!("Trade triggered Schwab execution with ID: {}", exec_id);

        let env_clone = env.clone();
        let pool_clone = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = execute_pending_schwab_execution(&env_clone, &pool_clone, exec_id).await
            {
                error!("Failed to execute Schwab order: {}", e);
            }
        });
    } else {
        info!(
            "Trade accumulated but did not trigger execution: tx_hash={:?}, log_index={}",
            tx_hash, log_index
        );
    }

    Ok(())
}

/// Execute a pending Schwab execution by fetching it from the database and placing the order.
async fn execute_pending_schwab_execution(
    env: &Env,
    pool: &SqlitePool,
    execution_id: i64,
) -> anyhow::Result<()> {
    let execution = find_execution_by_id(pool, execution_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Execution with ID {execution_id} not found"))?;

    info!("Executing Schwab order: {execution:?}");

    // Use the unified execute_schwab_order function with retry logic
    execute_schwab_order(env, pool, execution)
        .await
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::trade::OnchainTrade;
    use crate::schwab::Direction;
    use crate::test_utils::{OnchainTradeBuilder, setup_test_db};
    use alloy::primitives::{IntoLogData, U256, address, fixed_bytes, keccak256};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::rpc::types::Log;
    use alloy::sol_types::{self, SolCall, SolValue};
    use bindings::IERC20::symbolCall;
    use bindings::IOrderBookV4::{
        AfterClear, ClearConfig, ClearStateChange, ClearV2, TakeOrderConfigV3, TakeOrderV2,
    };
    use futures_util::stream;
    use serde_json::json;
    use std::str::FromStr;

    fn create_test_env_with_order_hash(order_hash: alloy::primitives::B256) -> Env {
        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            schwab_auth: SchwabAuthEnv {
                app_key: "test_key".to_string(),
                app_secret: "test_secret".to_string(),
                redirect_uri: "https://127.0.0.1".to_string(),
                base_url: "https://test.com".to_string(),
                account_index: 0,
            },
            evm_env: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1111111111111111111111111111111111111111"),
                order_hash,
                deployment_block: 1,
            },
        }
    }

    fn create_test_env() -> Env {
        create_test_env_with_order_hash(fixed_bytes!(
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ))
    }

    #[tokio::test]
    async fn test_step_returns_ok_when_partial_trade_is_none() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();

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
        let clear_stream_item: Result<(ClearV2, Log), sol_types::Error> = Ok((clear_event, log));
        let mut clear_stream = Box::pin(stream::once(async { clear_stream_item }));
        let mut take_stream = Box::pin(stream::empty());

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        step(
            &mut clear_stream,
            &mut take_stream,
            &env,
            &pool,
            &cache,
            &provider,
        )
        .await
        .unwrap();

        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_onchain_trade_duplicate_handling() {
        let pool = setup_test_db().await;

        let existing_trade = OnchainTradeBuilder::new()
            .with_tx_hash(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ))
            .with_log_index(293)
            .with_symbol("AAPLs1")
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
    async fn test_step_early_returns_on_duplicate_trade() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();

        let existing_trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            symbol: "AAPLs1".to_string(),
            amount: 5.0,
            direction: Direction::Sell,
            price_usdc: 20000.0,
            created_at: None,
        };
        let mut sql_tx = pool.begin().await.unwrap();
        existing_trade
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        cache.insert_for_test(
            address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "USDC".to_string(),
        );
        cache.insert_for_test(
            address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            "AAPLs1".to_string(),
        );

        let take_event = TakeOrderV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            config: TakeOrderConfigV3::default(),
            input: U256::default(),
            output: U256::default(),
        };
        let log = crate::test_utils::get_test_log();
        let take_stream_item: Result<(TakeOrderV2, Log), sol_types::Error> = Ok((take_event, log));
        let mut clear_stream = Box::pin(stream::empty());
        let mut take_stream = Box::pin(stream::once(async { take_stream_item }));

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        step(
            &mut clear_stream,
            &mut take_stream,
            &env,
            &pool,
            &cache,
            &provider,
        )
        .await
        .unwrap();

        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_step_creates_and_processes_new_trade() {
        let pool = setup_test_db().await;
        let order = crate::test_utils::get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let env = create_test_env_with_order_hash(order_hash);
        let cache = SymbolCache::default();

        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::from_str("9000000000000000000").unwrap(),
                bobOutput: U256::from(100_000_000u64),
                aliceInput: U256::from(100_000_000u64),
                bobInput: U256::from_str("9000000000000000000").unwrap(),
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        let clear_stream_item = Ok((clear_event, clear_log));
        let mut clear_stream = Box::pin(stream::once(async { clear_stream_item }));
        let mut take_stream = Box::pin(stream::empty());

        let asserter = Asserter::new();
        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        step(
            &mut clear_stream,
            &mut take_stream,
            &env,
            &pool,
            &cache,
            &provider,
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 1);

        let trade = OnchainTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 1)
            .await
            .unwrap();
        assert_eq!(trade.symbol, "AAPLs1");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON); // Expected amount from test data  
        assert!((trade.price_usdc - 11.111_111_111_111_11).abs() < 0.001); // Updated expected price
    }
}
