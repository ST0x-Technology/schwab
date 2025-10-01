use rocket::Config;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::{error, info};

pub mod api;
mod bindings;
pub mod cli;
mod conductor;
pub mod env;
mod error;
mod lock;
mod onchain;
mod pyth;
mod queue;
pub mod schwab;
mod symbol;
mod trade_execution_link;
mod trading_hours_controller;

#[cfg(test)]
pub mod test_utils;

use crate::conductor::run_market_hours_loop;
use crate::env::Env;
use crate::schwab::market_hours_cache::MarketHoursCache;
use crate::schwab::tokens::spawn_automatic_token_refresh;
use crate::trading_hours_controller::TradingHoursController;

pub async fn launch(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

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
    let token_refresher = spawn_automatic_token_refresh(pool.clone(), env.schwab_auth.clone());

    let market_hours_cache = Arc::new(MarketHoursCache::new());
    let controller = TradingHoursController::new(
        market_hours_cache,
        env.schwab_auth.clone(),
        Arc::new(pool.clone()),
    );

    run_market_hours_loop(controller, env, pool, token_refresher).await
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
