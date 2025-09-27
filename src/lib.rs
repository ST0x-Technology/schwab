use alloy::providers::{ProviderBuilder, WsConnect};
use futures_util::Stream;
use rocket::Config;
use sqlx::SqlitePool;
use tracing::{error, info, warn};

pub mod api;
mod bindings;
pub mod cli;
mod conductor;
mod db_utils;
pub mod env;
mod error;
mod lock;
mod offchain;
mod onchain;
mod queue;
mod symbol;
mod trade_execution_link;
mod trading_hours_controller;

#[cfg(test)]
pub mod test_utils;

use crate::conductor::get_cutoff_block;
use crate::env::Env;
use crate::symbol::cache::SymbolCache;
use crate::trading_hours_controller::TradingHoursController;
use bindings::IOrderBookV4::IOrderBookV4Instance;
use st0x_broker::Broker;
use st0x_broker::schwab::tokens::SchwabTokens;
use st0x_broker::schwab::{SchwabError, market_hours_cache::MarketHoursCache};

pub async fn launch(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    // Run database migrations to ensure all tables exist
    sqlx::migrate!().run(&pool).await?;

    let config = Config::figment()
        .merge(("port", env.server_port))
        .merge(("address", "0.0.0.0"));

    let rocket = rocket::custom(config)
        .mount("/", api::routes())
        .manage(pool.clone())
        .manage(env.clone());

    let server_task = tokio::spawn(rocket.launch());

    let bot_pool = pool.clone();
    let bot_task = tokio::spawn(async move {
        if let Err(e) = run(env, bot_pool).await {
            error!("Bot failed: {e}");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal, shutting down gracefully...");
        }

        result = server_task => {
            match result {
                Ok(Ok(_)) => info!("Server completed successfully"),
                Ok(Err(e)) => error!("Server failed: {e}"),
                Err(e) => error!("Server task panicked: {e}"),
            }
        }

        result = bot_task => {
            match result {
                Ok(()) => info!("Bot task completed"),
                Err(e) => error!("Bot task panicked: {e}"),
            }
        }
    }

    info!("Shutdown complete");
    Ok(())
}

async fn run(env: Env, pool: SqlitePool) -> anyhow::Result<()> {
    const RERUN_DELAY_SECS: u64 = 10;

    loop {
        let result = run_bot_session(&env, &pool).await;

        match result {
            Ok(()) => {
                info!("Bot session completed successfully");
                break Ok(());
            }
            Err(e) if e.to_string().contains("RefreshTokenExpired") => {
                warn!(
                    "Refresh token expired, retrying in {} seconds",
                    RERUN_DELAY_SECS
                );
                tokio::time::sleep(std::time::Duration::from_secs(RERUN_DELAY_SECS)).await;
            }
            Err(e) => {
                error!("Bot session failed: {e}");
                return Err(e);
            }
        }
    }
}

async fn run_bot_session(env: &Env, pool: &SqlitePool) -> anyhow::Result<()> {
    if env.dry_run {
        info!("Initializing test broker for dry-run mode");
        let broker = env.get_test_broker().await?;
        run_with_broker(env.clone(), pool.clone(), broker).await
    } else {
        info!("Initializing Schwab broker");
        let broker = env.get_schwab_broker(pool.clone()).await?;
        run_with_broker(env.clone(), pool.clone(), broker).await
    }
}

async fn run_with_broker<B: Broker + Clone>(
    env: Env,
    pool: SqlitePool,
    broker: B,
) -> anyhow::Result<()> {
    if let Some(wait_duration) = broker
        .wait_until_market_open()
        .await
        .map_err(|e| anyhow::anyhow!("Market hours check failed: {}", e))?
    {
        info!(
            "Market is closed, waiting {} minutes until market opens",
            wait_duration.as_secs() / 60
        );
        tokio::time::sleep(wait_duration).await;
    }

    info!("Market is open, starting bot session");

    let (provider, cache, mut clear_stream, mut take_stream) =
        initialize_event_streams(env.clone()).await?;

    let cutoff_block =
        get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, &pool).await?;

    onchain::backfill::backfill_events(&pool, &provider, &env.evm_env, cutoff_block - 1).await?;

    conductor::run_live(
        env.clone(),
        pool.clone(),
        cache,
        provider,
        broker,
        clear_stream,
        take_stream,
    )
    .await
}

async fn initialize_event_streams(
    env: Env,
) -> anyhow::Result<(
    impl alloy::providers::Provider + Clone,
    SymbolCache,
    impl Stream<
        Item = Result<
            (bindings::IOrderBookV4::ClearV2, alloy::rpc::types::Log),
            alloy::sol_types::Error,
        >,
    >,
    impl Stream<
        Item = Result<
            (bindings::IOrderBookV4::TakeOrderV2, alloy::rpc::types::Log),
            alloy::sol_types::Error,
        >,
    >,
)> {
    let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let cache = SymbolCache::default();
    let orderbook = IOrderBookV4Instance::new(env.evm_env.orderbook, &provider);

    let clear_stream = orderbook.ClearV2_filter().watch().await?.into_stream();
    let take_stream = orderbook.TakeOrderV2_filter().watch().await?.into_stream();

    Ok((provider, cache, clear_stream, take_stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::tests::create_test_env;

    async fn create_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_run_function_websocket_connection_error() {
        let mut env = create_test_env();
        let pool = create_test_pool().await;
        env.evm_env.ws_rpc_url = "ws://invalid.nonexistent.url:8545".parse().unwrap();
        run(env, pool).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_invalid_orderbook_address() {
        let mut env = create_test_env();
        let pool = create_test_pool().await;
        env.evm_env.orderbook = alloy::primitives::Address::ZERO;
        env.evm_env.ws_rpc_url = "ws://localhost:8545".parse().unwrap();
        run(env, pool).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_error_propagation() {
        let mut env = create_test_env();
        env.database_url = "invalid://database/url".to_string();
        let pool = create_test_pool().await;
        run(env, pool).await.unwrap_err();
    }
}
