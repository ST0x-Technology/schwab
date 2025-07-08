use alloy::primitives::{Address, B256};
use alloy::providers::{ProviderBuilder, WsConnect};
use clap::Parser;
use futures_util::StreamExt;
use sqlx::SqlitePool;

mod bindings;
pub mod schwab_auth;
mod symbol_cache;
mod trade;

#[cfg(test)]
pub mod test_utils;

use bindings::IOrderBookV4::IOrderBookV4Instance;
use symbol_cache::SymbolCache;
use trade::Trade;

#[derive(Parser, Debug)]
pub struct Env {
    #[clap(short, long, env)]
    pub database_url: String,
    #[clap(short, long, env)]
    pub ws_rpc_url: url::Url,
    #[clap(short = 'b', long, env)]
    pub orderbook: Address,
    #[clap(short, long, env)]
    pub order_hash: B256,
}

pub async fn run(env: Env, _pool: &SqlitePool) -> anyhow::Result<()> {
    let ws = WsConnect::new(env.ws_rpc_url.as_str());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let cache = SymbolCache::default();
    let orderbook = IOrderBookV4Instance::new(env.orderbook, &provider);

    let clear_filter = orderbook.ClearV2_filter().watch().await?;
    let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    let mut clear_stream = clear_filter.into_stream();
    let mut take_stream = take_filter.into_stream();

    loop {
        let trade = tokio::select! {
            Some(next_res) = clear_stream.next() => {
                let (event, log) = next_res?;
                Trade::try_from_clear_v2(&env, &cache, &provider, event, log).await?
            }
            Some(take) = take_stream.next() => {
                let (event, log) = take?;
                Trade::try_from_take_order_if_target_order(&cache, &provider, event, log, env.order_hash).await?
            }
        };

        if let Some(trade) = trade {
            println!("TODO: dedup and trade: {trade:?}");
        }
    }
}
