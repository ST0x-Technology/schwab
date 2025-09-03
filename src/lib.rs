use alloy::providers::{ProviderBuilder, WsConnect};
use backon::{ConstantBuilder, Retryable};
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

#[cfg(test)]
pub mod test_utils;

use crate::conductor::get_cutoff_block;
use crate::env::Env;
use crate::schwab::SchwabError;
use crate::symbol::cache::SymbolCache;
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
    let run_bot = || async {
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

        conductor::process_queue(&env, &pool, &cache, &provider).await?;

        // Spawn the queue processor service
        let queue_processor_handle = {
            let env_clone = env.clone();
            let pool_clone = pool.clone();
            let cache_clone = cache.clone();
            let provider_clone = provider.clone();

            tokio::spawn(async move {
                if let Err(e) = conductor::run_queue_processor(
                    &env_clone,
                    &pool_clone,
                    &cache_clone,
                    provider_clone,
                )
                .await
                {
                    error!("Queue processor service failed: {e}");
                }
            })
        };

        // Spawn the live event listeners
        let live_handle = tokio::spawn({
            let env_clone = env.clone();
            let pool_clone = pool.clone();

            async move { conductor::run_live(env_clone, pool_clone, clear_stream, take_stream).await }
        });

        // Wait for services - if any terminates, we restart the whole bot
        tokio::select! {
            _ = queue_processor_handle => {
                error!("Queue processor service terminated unexpectedly");
                Err(anyhow::anyhow!("Queue processor terminated"))
            }
            result = live_handle => {
                match result {
                    Ok(Ok(())) => {
                        error!("Live event listener terminated unexpectedly");
                        Err(anyhow::anyhow!("Live event listener terminated"))
                    }
                    Ok(Err(e)) => {
                        error!("Live event listener failed: {e}");
                        Err(e)
                    }
                    Err(e) => {
                        error!("Live event listener task panicked: {e}");
                        Err(anyhow::anyhow!("Live event listener panicked: {e}"))
                    }
                }
            }
        }
    };

    const RERUN_DELAY_SECS: u64 = 10;

    run_bot
        .retry(
            ConstantBuilder::default()
                .with_delay(std::time::Duration::from_secs(RERUN_DELAY_SECS))
                .with_max_times(usize::MAX), // Retry indefinitely
        )
        .when(|e| {
            if let Some(msg) = e.downcast_ref::<String>() {
                if msg == "RefreshTokenExpired" {
                    info!("Retrying in {RERUN_DELAY_SECS} seconds due to expired refresh token - waiting for manual authentication");
                    return true;
                }
            }
            if e.to_string().contains("RefreshTokenExpired") {
                info!("Retrying in 30 seconds due to expired refresh token - waiting for manual authentication");
                true
            } else {
                false
            }
        })
        .await
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
