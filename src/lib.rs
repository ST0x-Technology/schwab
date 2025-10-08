use rocket::Config;
use sqlx::SqlitePool;
use tracing::{error, info};

use st0x_broker::Broker;

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

#[cfg(test)]
pub mod test_utils;

use crate::conductor::run_market_hours_loop;
use crate::env::Env;

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
    if env.dry_run {
        let broker = env.get_test_broker().await?;
        let broker_maintenance = broker.run_broker_maintenance().await;
        run_market_hours_loop(broker, env, pool, broker_maintenance).await
    } else {
        let broker = env.get_schwab_broker(pool.clone()).await?;
        let broker_maintenance = broker.run_broker_maintenance().await;
        run_market_hours_loop(broker, env, pool, broker_maintenance).await
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
