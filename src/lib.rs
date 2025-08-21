use alloy::providers::{ProviderBuilder, WsConnect};
use backon::{ConstantBuilder, Retryable};
use tracing::{debug, info, warn};

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

pub async fn run(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

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

        conductor::process_queue(&env, &env.evm_env, &pool, &cache, &provider).await?;

        conductor::run_live(
            env.clone(),
            pool.clone(),
            cache,
            provider,
            clear_stream,
            take_stream,
        )
        .await
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

    #[tokio::test]
    async fn test_run_function_websocket_connection_error() {
        let mut env = create_test_env();
        env.evm_env.ws_rpc_url = "ws://invalid.nonexistent.url:8545".parse().unwrap();
        run(env).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_invalid_orderbook_address() {
        let mut env = create_test_env();
        env.evm_env.orderbook = alloy::primitives::Address::ZERO;
        env.evm_env.ws_rpc_url = "ws://localhost:8545".parse().unwrap();
        run(env).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_error_propagation() {
        let mut env = create_test_env();
        env.database_url = "invalid://database/url".to_string();
        run(env).await.unwrap_err();
    }
}
