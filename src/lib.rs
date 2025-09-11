use alloy::providers::{ProviderBuilder, WsConnect};
use rocket::Config;
use sqlx::SqlitePool;
use tracing::{debug, error, info, warn};

pub mod api;
mod bindings;
pub mod cli;
mod conductor;
pub mod env;
mod error;
mod lock;
mod onchain;
mod queue;
pub mod schwab;
mod symbol;
mod trade_execution_link;
mod trading_hours_controller;

#[cfg(test)]
pub mod test_utils;

use crate::conductor::get_cutoff_block;
use crate::env::Env;
use crate::schwab::SchwabError;
use crate::schwab::market_hours_cache::MarketHoursCache;
use crate::symbol::cache::SymbolCache;
use crate::trading_hours_controller::TradingHoursController;
use bindings::IOrderBookV4::IOrderBookV4Instance;

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

fn create_bot_runner(
    env: Env,
    pool: SqlitePool,
) -> impl Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = anyhow::Result<conductor::BackgroundTasks>> + Send>,
> + Send {
    move || {
        let env = env.clone();
        let pool = pool.clone();
        Box::pin(async move {
            debug!("Validating Schwab tokens...");
            match schwab::tokens::SchwabTokens::refresh_if_needed(&pool, &env.schwab_auth).await {
                Err(SchwabError::RefreshTokenExpired) => {
                    warn!("Refresh token expired, waiting for manual authentication via API");
                    return Err(anyhow::anyhow!("RefreshTokenExpired"));
                }
                Err(e) => return Err(anyhow::anyhow!("Token validation failed: {}", e)),
                Ok(_) => {
                    info!("Token validation successful");
                }
            }

            let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
            let provider = ProviderBuilder::new().connect_ws(ws).await?;
            let cache = SymbolCache::default();
            let orderbook = IOrderBookV4Instance::new(env.evm_env.orderbook, &provider);

            schwab::tokens::SchwabTokens::spawn_automatic_token_refresh(
                pool.clone(),
                env.schwab_auth.clone(),
            );

            let mut clear_stream = orderbook.ClearV2_filter().watch().await?.into_stream();
            let mut take_stream = orderbook.TakeOrderV2_filter().watch().await?.into_stream();

            let cutoff_block =
                get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, &pool).await?;

            onchain::backfill::backfill_events(&pool, &provider, &env.evm_env, cutoff_block - 1)
                .await?;

            // Start all services through unified background tasks management
            Ok(conductor::run_live(
                &env,
                &pool,
                cache,
                provider,
                clear_stream,
                take_stream,
            ))
        })
    }
}

async fn run_market_hours_loop(
    controller: TradingHoursController,
    run_bot: impl Fn() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<conductor::BackgroundTasks>> + Send>,
    >,
) -> anyhow::Result<()> {
    const RERUN_DELAY_SECS: u64 = 10;

    // Main market hours control loop
    loop {
        // Wait until market opens
        controller.wait_until_market_open().await?;

        // Run bot until market closes or completes
        let run_result =
            if let Some(time_until_close) = controller.time_until_market_close().await? {
                let timeout_duration = time_until_close
                    .to_std()
                    .unwrap_or(std::time::Duration::from_secs(60 * 60)); // 1 hour fallback

                info!(
                    "Market is open, starting bot (will timeout in {} minutes)",
                    timeout_duration.as_secs() / 60
                );

                // Run the bot and get background tasks handle
                let bot_result = run_bot().await;
                match bot_result {
                    Ok(mut background_tasks) => {
                        info!("Market opened, starting backfilling and live trading services");

                        tokio::select! {
                            result = background_tasks.wait_for_completion() => {
                                info!("All background tasks completed");
                                result
                            }
                            () = tokio::time::sleep(timeout_duration) => {
                                info!("Market closed, shutting down all background tasks");
                                background_tasks.abort_all();
                                info!("All tasks shutdown. Backfill will start on market open");
                                continue; // Go back to wait for next market open
                            }
                        }
                    }
                    Err(e) => Err(e),
                }
            } else {
                // Market already closed, continue to wait
                warn!("Market already closed, waiting for next open");
                continue;
            };

        // Handle bot result - simple retry for token expired, otherwise fail
        match run_result {
            Ok(()) => {
                info!("Bot completed successfully, continuing to next market session");
            }
            Err(e) if e.to_string().contains("RefreshTokenExpired") => {
                warn!(
                    "Refresh token expired, retrying in {} seconds",
                    RERUN_DELAY_SECS
                );
                tokio::time::sleep(std::time::Duration::from_secs(RERUN_DELAY_SECS)).await;
            }
            Err(e) => {
                error!("Bot failed: {e}");
                return Err(e);
            }
        }
    }
}

async fn run(env: Env, pool: SqlitePool) -> anyhow::Result<()> {
    let run_bot = create_bot_runner(env.clone(), pool.clone());

    // Initialize market hours controller
    let market_hours_cache = std::sync::Arc::new(MarketHoursCache::new());
    let controller = TradingHoursController::new(
        market_hours_cache,
        env.schwab_auth.clone(),
        std::sync::Arc::new(pool.clone()),
    );

    run_market_hours_loop(controller, run_bot).await
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

    #[tokio::test]
    async fn test_market_close_timeout_simulation() {
        use crate::conductor;
        use crate::symbol::cache::SymbolCache;
        use alloy::providers::mock::Asserter;
        use futures_util::stream;
        use std::time::Duration;

        let pool = create_test_pool().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        // Create empty streams (simulating no events)
        let clear_stream = stream::empty();
        let take_stream = stream::empty();

        // Simulate getting BackgroundTasks from run_live
        let mut background_tasks =
            conductor::run_live(&env, &pool, cache, provider, clear_stream, take_stream);

        // Simulate market close timeout scenario
        let timeout_duration = Duration::from_millis(50); // Very short for testing

        let start_time = std::time::Instant::now();

        // This simulates the tokio::select! pattern in the main lib.rs logic
        tokio::select! {
            _ = background_tasks.wait_for_completion() => {
                panic!("Tasks should not complete before timeout");
            }
            () = tokio::time::sleep(timeout_duration) => {
                let elapsed = start_time.elapsed();
                assert!(elapsed >= timeout_duration, "Timeout should have elapsed");
                assert!(elapsed < timeout_duration + Duration::from_millis(20), "Should timeout quickly");

                // This is where the market close logic would abort tasks
                background_tasks.abort_all();
                // Test passes if abort_all completes without panic
            }
        }
    }

    #[tokio::test]
    async fn test_background_tasks_wait_vs_abort_race() {
        use crate::conductor;
        use crate::symbol::cache::SymbolCache;
        use alloy::providers::mock::Asserter;
        use futures_util::stream;
        use std::time::Duration;

        let pool = create_test_pool().await;
        let env = create_test_env();
        let cache = SymbolCache::default();
        let asserter = Asserter::new();
        let provider = alloy::providers::ProviderBuilder::new().connect_mocked_client(asserter);

        let clear_stream = stream::empty();
        let take_stream = stream::empty();

        let mut background_tasks =
            conductor::run_live(&env, &pool, cache, provider, clear_stream, take_stream);

        // Test the race condition between wait_for_completion and abort
        // This simulates what happens when market closes during normal operation

        let wait_future = background_tasks.wait_for_completion();
        let timeout = tokio::time::sleep(Duration::from_millis(10));

        tokio::select! {
            result = wait_future => {
                // Tasks completed before timeout (should not happen with infinite loops)
                assert!(result.is_ok(), "wait_for_completion should succeed even if tasks are aborted");
            }
            () = timeout => {
                // Timeout occurred first, abort tasks (this is the expected path)
                background_tasks.abort_all();
            }
        }
    }
}
