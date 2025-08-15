use alloy::primitives::{Address, B256};
use clap::Parser;

pub mod accumulator;
mod clear;
pub mod position_calculator;
mod take_order;
pub mod trade;
pub mod trade_execution_link;

pub use trade::OnchainTrade;
pub use trade_execution_link::{
    AuditTrailEntry, ExecutionContribution, TradeContribution, TradeExecutionLink,
};

#[derive(Parser, Debug, Clone)]
pub struct EvmEnv {
    #[clap(short, long, env)]
    pub ws_rpc_url: url::Url,
    #[clap(short = 'b', long, env)]
    pub orderbook: Address,
    #[clap(short, long, env)]
    pub order_hash: B256,
}
