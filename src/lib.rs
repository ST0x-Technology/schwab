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
        .merge(("port", 8080))
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
    let run_bot = {
        let env = env.clone();
        let pool = pool.clone();
        move || {
            let env = env.clone();
            let pool = pool.clone();
            async move {
                debug!("Validating Schwab tokens...");
                match schwab::tokens::SchwabTokens::refresh_if_needed(&pool, &env.schwab_auth).await
                {
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

                onchain::backfill::backfill_events(
                    &pool,
                    &provider,
                    &env.evm_env,
                    cutoff_block - 1,
                )
                .await?;

                // Start all services through unified background tasks management
                conductor::run_live(env, pool, cache, provider, clear_stream, take_stream).await
            }
        }
    };

    const RERUN_DELAY_SECS: u64 = 10;

    // Initialize market hours controller
    let market_hours_cache = std::sync::Arc::new(MarketHoursCache::new());
    let controller = TradingHoursController::new(
        market_hours_cache,
        env.schwab_auth.clone(),
        std::sync::Arc::new(pool.clone()),
    );

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

                tokio::select! {
                    result = run_bot() => result,
                    () = tokio::time::sleep(timeout_duration) => {
                        info!("Market closing, shutting down bot gracefully");
                        continue; // Go back to wait for next market open
                    }
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
