use alloy::primitives::{Address, B256};
use alloy::sol;
use clap::Parser;
use sqlx::SqlitePool;

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

sol!(
    #![sol(all_derives = true, rpc)]
    IOrderBookV4, "lib/rain.orderbook.interface/out/IOrderBookV4.sol/IOrderBookV4.json"
);

pub async fn run(_env: Env, _pool: SqlitePool) -> anyhow::Result<()> {
    Ok(())
}
