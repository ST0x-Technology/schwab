use std::collections::BTreeMap;

use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{Address, B256};
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::sol;
use clap::Parser;
use futures_util::StreamExt;
use lazy_static::lazy_static;
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

sol!(
    #![sol(all_derives = true, rpc)]
    IERC20, "lib/forge-std/out/IERC20.sol/IERC20.json"
);

use IERC20::IERC20Instance;
use IOrderBookV4::{IOrderBookV4Instance, TakeOrderConfigV3, TakeOrderV2};

struct Trade {
    onchain_input_symbol: String,
}

#[derive(Debug, thiserror::Error)]
enum TradeConversionError {
    #[error("Invalid input index: {0}")]
    InvalidInputIndex(FromUintError<usize>),
    #[error("No input found at index: {0}")]
    NoInputAtIndex(usize),
    #[error("Failed to get symbol: {0}")]
    GetSymbol(alloy::contract::Error),
}

lazy_static! {
    static ref SYMBOL_MAP: BTreeMap<Address, String> = { BTreeMap::new() };
}

impl Trade {
    async fn try_from_take_order<P: Provider>(
        provider: P,
        event: TakeOrderV2,
    ) -> Result<Self, TradeConversionError> {
        let TakeOrderConfigV3 {
            order,
            inputIOIndex,
            outputIOIndex,
            signedContext,
        } = event.config;

        let input_index = usize::try_from(inputIOIndex)?;
        let input = order
            .validInputs
            .iter()
            .nth(input_index)
            .ok_or(TradeConversionError::NoInputAtIndex(input_index))?;

        let input_symbol = if let Some(symbol) = SYMBOL_MAP.get(&input.token) {
            symbol.to_owned()
        } else {
            let erc20 = IERC20Instance::new(input.token, provider);
            let symbol = erc20.symbol().call().await?;
            SYMBOL_MAP.insert(input.token, symbol);
            symbol
        };

        Ok(Trade {
            onchain_input_symbol: input_symbol,
        })
    }
}

// impl From<ClearV2> for Trade {
//     fn from(event: ClearV2) -> Self {
//         Trade {
//             onchain_input_symbol: event.clearConfig.,
//         }
//     }
// }

pub async fn run(env: Env, pool: &SqlitePool) -> anyhow::Result<()> {
    let ws = WsConnect::new(env.ws_rpc_url);
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let orderbook = IOrderBookV4Instance::new(env.orderbook, provider);

    let clear_filter = orderbook.ClearV2_filter().watch().await?;
    // let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    let mut clear_stream = clear_filter.into_stream();
    // let mut take_stream = take_filter.into_stream();

    loop {
        let trade = tokio::select! {
            Some(next_res) = clear_stream.next() => {
                let (event, _) = next_res?;

                Trade::from(event)
            }
            // Some(log) = take_stream.next() => {
            //     println!("{log:?}");
            // }
        };
    }
}
