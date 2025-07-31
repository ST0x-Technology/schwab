use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::rpc::types::Log;
use alloy::sol_types;
use backon::{ExponentialBuilder, Retryable};
use clap::Parser;
use futures_util::{Stream, StreamExt};
use sqlx::SqlitePool;
use tracing::{error, info};

pub mod arb;
mod bindings;
pub mod schwab;
mod symbol_cache;
pub mod trade;

#[cfg(test)]
pub mod test_utils;

use arb::ArbTrade;
use bindings::IOrderBookV4::{ClearV2, IOrderBookV4Instance, TakeOrderV2};
use schwab::{
    SchwabAuthEnv,
    order::{Instruction, Order},
};
use symbol_cache::SymbolCache;
use trade::{EvmEnv, PartialArbTrade, SchwabInstruction, TradeStatus};

#[derive(Parser, Debug, Clone)]
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

pub fn setup_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rain_schwab=debug,auth=debug,main=debug".into()),
        )
        .init();
}

pub async fn run(env: Env) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    info!("Validating Schwab tokens...");
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

    let clear_filter = orderbook.ClearV2_filter().watch().await?;
    let take_filter = orderbook.TakeOrderV2_filter().watch().await?;

    let mut clear_stream = clear_filter.into_stream();
    let mut take_stream = take_filter.into_stream();

    loop {
        step(
            &mut clear_stream,
            &mut take_stream,
            &env,
            &pool,
            &cache,
            &provider,
        )
        .await?;
    }
}

async fn step<S1, S2, P>(
    clear_stream: &mut S1,
    take_stream: &mut S2,
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: &P,
) -> anyhow::Result<()>
where
    S1: Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin,
    S2: Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin,
    P: Provider + Clone,
{
    let trade = tokio::select! {
        Some(next_res) = clear_stream.next() => {
            let (event, log) = next_res?;
            PartialArbTrade::try_from_clear_v2(&env.evm_env, cache, provider, event, log).await?
        }
        Some(take) = take_stream.next() => {
            let (event, log) = take?;
            PartialArbTrade::try_from_take_order_if_target_order(cache, provider, event, log, env.evm_env.order_hash).await?
        }
    };

    let Some(trade) = trade else {
        return Ok(());
    };

    if ArbTrade::exists_in_db(pool, trade.tx_hash, trade.log_index).await? {
        info!(
            "Trade already exists in database, skipping: tx_hash={tx_hash:?}, log_index={log_index}",
            tx_hash = trade.tx_hash,
            log_index = trade.log_index
        );
        return Ok(());
    }

    let arb_trade = ArbTrade::from_partial_trade(trade.clone());
    arb_trade.save_to_db(pool).await?;
    info!("Saved trade to database: {trade:?}");

    let env_clone = env.clone();
    let pool_clone = pool.clone();

    tokio::spawn(async move {
        execute_schwab_order(env_clone, pool_clone, arb_trade).await;
    });

    Ok(())
}
            }
        };

        if let Some(trade) = trade {
            if Trade::exists_in_db(&pool, trade.tx_hash, trade.log_index).await? {
                tracing::info!(
                    "Trade already exists in database, skipping: tx_hash={:?}, log_index={}",
                    trade.tx_hash,
                    trade.log_index
                );
                continue;
            }

            trade.save_to_db(&pool).await?;
            tracing::info!("Saved trade to database: {:?}", trade);
        }
    }
}
