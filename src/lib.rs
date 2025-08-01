use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::Log;
use alloy::sol_types;
use clap::Parser;
use futures_util::{Stream, StreamExt};
use sqlx::SqlitePool;
use tracing::{Level, info};

pub mod arb;
mod bindings;
pub mod schwab;
mod symbol_cache;
pub mod trade;

#[cfg(test)]
pub mod test_utils;

use arb::ArbTrade;
use bindings::IOrderBookV4::{ClearV2, IOrderBookV4Instance, TakeOrderV2};
use schwab::{SchwabAuthEnv, order::execute_trade};
use symbol_cache::SymbolCache;
use trade::{EvmEnv, PartialArbTrade};

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

#[derive(Parser, Debug, Clone)]
pub struct Env {
    #[clap(short, long, env)]
    pub database_url: String,
    #[clap(short = 'l', long, env, default_value = "debug")]
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

pub fn setup_tracing(log_level: LogLevel) {
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
    let trade = tokio::select! {
        Some(next_res) = clear_stream.next() => {
            let (event, log) = next_res?;
            PartialArbTrade::try_from_clear_v2(&env.evm_env, cache, provider, event, log).await?
        }
        Some(take) = take_stream.next() => {
            let (event, log) = take?;
            PartialArbTrade::try_from_take_order_if_target_order(cache, provider, event, log, env.evm_env.order_hash).await?
        }
    };

    let Some(trade) = trade else {
        return Ok(());
    };

    let arb_trade = ArbTrade::from_partial_trade(trade.clone());

    let was_inserted = arb_trade.try_save_to_db(pool).await?;
    if !was_inserted {
        info!(
            "Trade already exists in database, skipping: tx_hash={tx_hash:?}, log_index={log_index}",
            tx_hash = trade.tx_hash,
            log_index = trade.log_index
        );
        return Ok(());
    }

    info!("Saved trade to database: {trade:?}");

    let env_clone = env.clone();
    let pool_clone = pool.clone();

    const MAX_TRADE_RETRIES: usize = 10;
    tokio::spawn(async move {
        execute_trade(&env_clone, &pool_clone, arb_trade, MAX_TRADE_RETRIES).await;
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trade::{SchwabInstruction, TradeStatus};
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
    use sqlx::SqlitePool;
    use std::str::FromStr;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

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

        let count = sqlx::query!("SELECT COUNT(*) as count FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.count, 0);
    }

    #[tokio::test]
    async fn test_arb_trade_duplicate_handling() {
        let pool = setup_test_db().await;

        let existing_trade = ArbTrade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            id: None,
            created_at: None,
            completed_at: None,
        };
        existing_trade.try_save_to_db(&pool).await.unwrap();

        let duplicate_trade = existing_trade.clone();
        let was_inserted = duplicate_trade.try_save_to_db(&pool).await.unwrap();
        assert!(!was_inserted);

        let count = sqlx::query!("SELECT COUNT(*) as count FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.count, 1);
    }

    #[tokio::test]
    async fn test_step_early_returns_on_duplicate_trade() {
        let pool = setup_test_db().await;
        let env = create_test_env();
        let cache = SymbolCache::default();

        let existing_trade = ArbTrade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            id: None,
            created_at: None,
            completed_at: None,
        };
        existing_trade.try_save_to_db(&pool).await.unwrap();

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

        let count = sqlx::query!("SELECT COUNT(*) as count FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.count, 1);
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

        let count = sqlx::query!("SELECT COUNT(*) as count FROM trades")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.count, 1);

        let trade = sqlx::query!("SELECT * FROM trades LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(trade.onchain_input_symbol.unwrap(), "USDC");
        assert_eq!(trade.onchain_output_symbol.unwrap(), "AAPLs1");
        assert_eq!(trade.schwab_ticker.unwrap(), "AAPL");
        assert_eq!(trade.schwab_instruction.unwrap(), "BUY");
        assert!(trade.schwab_price_per_share_cents.is_none());
        assert_eq!(trade.status.unwrap(), "PENDING");
    }
}
