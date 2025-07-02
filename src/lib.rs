use alloy::sol;
use clap::Parser;

#[derive(Parser, Debug)]
pub struct Env {
    #[clap(short, long, env)]
    pub ws_rpc_url: String,
}

sol!(
    #![sol(all_derives = true, rpc)]
    IOrderBookV4, "lib/rain.orderbook.interface/out/IOrderBookV4.sol/IOrderBookV4.json"
);
