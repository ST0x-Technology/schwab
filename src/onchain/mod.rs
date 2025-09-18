use alloy::primitives::Address;
use clap::Parser;

pub(crate) mod accumulator;
pub(crate) mod backfill;
mod clear;
pub(crate) mod position_calculator;
mod take_order;
pub(crate) mod trade;
pub(crate) mod trade_execution_link;

pub use trade::OnchainTrade;

#[derive(Parser, Debug, Clone)]
pub struct EvmEnv {
    #[clap(short, long, env)]
    pub ws_rpc_url: url::Url,
    #[clap(short = 'b', long, env)]
    pub orderbook: Address,
    #[clap(short, long, env)]
    pub order_owner: Address,
    #[clap(short = 'd', long, env)]
    pub deployment_block: u64,
}
