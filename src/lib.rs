use alloy::providers::{ProviderBuilder, WsConnect};
use clap::Parser;
use futures_util::StreamExt;
use sqlx::SqlitePool;

mod bindings;
pub mod schwab;
mod symbol_cache;
pub mod trade;

#[cfg(test)]
pub mod test_utils;

use bindings::IOrderBookV4::IOrderBookV4Instance;
use schwab::SchwabAuthEnv;
use symbol_cache::SymbolCache;
use trade::{EvmEnv, Trade};

#[derive(Parser, Debug)]
pub struct Env {
    #[clap(short, long, env)]
    pub database_url: String,
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

pub async fn run(env: Env) -> anyhow::Result<()> {
    let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let cache = SymbolCache::default();
    let orderbook = IOrderBookV4Instance::new(env.evm_env.orderbook, &provider);

    let clear_filter = orderbook.ClearV2_filter().watch().await?;
    let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    let mut clear_stream = clear_filter.into_stream();
    let mut take_stream = take_filter.into_stream();

    loop {
        let trade = tokio::select! {
            Some(next_res) = clear_stream.next() => {
                let (event, log) = next_res?;
                Trade::try_from_clear_v2(&env.evm_env, &cache, &provider, event, log).await?
            }
            Some(take) = take_stream.next() => {
                let (event, log) = take?;
                Trade::try_from_take_order_if_target_order(&cache, &provider, event, log, env.evm_env.order_hash).await?
            }
        };

        if let Some(trade) = trade {
            println!("TODO: dedup and trade: {trade:?}");
        }
    }
}
