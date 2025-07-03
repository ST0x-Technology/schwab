use alloy::primitives::{Address, B256};
use alloy::providers::{ProviderBuilder, WsConnect};
use clap::Parser;
use futures_util::StreamExt;
use sqlx::SqlitePool;

mod bindings;
mod trade;

use bindings::IOrderBookV4::IOrderBookV4Instance;
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
    let ws = WsConnect::new(env.ws_rpc_url);
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let orderbook = IOrderBookV4Instance::new(env.orderbook, &provider);

    // let clear_filter = orderbook.ClearV2_filter().watch().await?;
    let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    // let mut clear_stream = clear_filter.into_stream();
    let mut take_stream = take_filter.into_stream();

    loop {
        let _trade = tokio::select! {
            // Some(next_res) = clear_stream.next() => {
            //     let (event, _) = next_res?;

            //     Trade::try_from_take_order(provider, event).await?
            // }
            Some(log) = take_stream.next() => {
                let (event, _) = log?;
                Trade::try_from_take_order(&provider, event).await?
            }
        };
    }
}
