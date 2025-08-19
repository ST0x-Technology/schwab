use alloy::providers::{ProviderBuilder, WsConnect};
use tracing::{debug, info};

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
use crate::symbol::cache::SymbolCache;
use bindings::IOrderBookV4::IOrderBookV4Instance;

pub async fn run(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    debug!("Validating Schwab tokens...");
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

    let mut clear_stream = orderbook.ClearV2_filter().watch().await?.into_stream();
    let mut take_stream = orderbook.TakeOrderV2_filter().watch().await?.into_stream();

    let cutoff_block =
        get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, &pool).await?;

    onchain::backfill::backfill_events(&pool, &provider, &env.evm_env, cutoff_block - 1).await?;

    conductor::process_queue(&env, &env.evm_env, &pool, &cache, &provider).await?;

    conductor::run_live(env, pool, cache, provider, clear_stream, take_stream).await
}
