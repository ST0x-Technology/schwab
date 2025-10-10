use sqlx::SqlitePool;
use tracing::{error, info, warn};

pub mod api;
mod bindings;
pub mod cli;
mod conductor;
pub mod env;
mod error;
mod lock;
mod offchain;
mod onchain;
mod queue;
mod symbol;
mod trade_execution_link;

#[cfg(test)]
pub mod test_utils;

use crate::env::{BrokerConfig, Config};
use st0x_broker::{Broker, MockBrokerConfig, SchwabConfig, TryIntoBroker};

pub async fn launch(config: Config) -> anyhow::Result<()> {
    let pool = config.get_sqlite_pool().await?;

    sqlx::migrate!().run(&pool).await?;

    let rocket_config = rocket::Config::figment()
        .merge(("port", config.server_port))
        .merge(("address", "0.0.0.0"));

    let rocket = rocket::custom(rocket_config)
        .mount("/", api::routes())
        .manage(pool.clone())
        .manage(config.clone());

    let server_task = tokio::spawn(rocket.launch());

    let bot_pool = pool.clone();
    let bot_task = tokio::spawn(async move {
        if let Err(e) = run(config, bot_pool).await {
            error!("Bot failed: {e}");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received shutdown signal, shutting down gracefully...");
        }

        result = server_task => {
            match result {
                Ok(Ok(_)) => info!("Server completed successfully"),
                Ok(Err(e)) => error!("Server failed: {e}"),
                Err(e) => error!("Server task panicked: {e}"),
            }
        }

        result = bot_task => {
            match result {
                Ok(()) => info!("Bot task completed"),
                Err(e) => error!("Bot task panicked: {e}"),
            }
        }
    }

    info!("Shutdown complete");
    Ok(())
}

async fn run(config: Config, pool: SqlitePool) -> anyhow::Result<()> {
    const RERUN_DELAY_SECS: u64 = 10;

    loop {
        let result = run_bot_session(&config, &pool).await;

        match result {
            Ok(()) => {
                info!("Bot session completed successfully");
                break Ok(());
            }
            Err(e) if e.to_string().contains("RefreshTokenExpired") => {
                warn!(
                    "Refresh token expired, retrying in {} seconds",
                    RERUN_DELAY_SECS
                );
                tokio::time::sleep(std::time::Duration::from_secs(RERUN_DELAY_SECS)).await;
            }
            Err(e) => {
                error!("Bot session failed: {e}");
                return Err(e);
            }
        }
    }
}

async fn run_bot_session(config: &Config, pool: &SqlitePool) -> anyhow::Result<()> {
    match &config.broker {
        BrokerConfig::DryRun => {
            info!("Initializing test broker for dry-run mode");
            let broker = MockBrokerConfig.try_into_broker().await?;
            run_with_broker(config.clone(), pool.clone(), broker).await
        }
        BrokerConfig::Schwab(schwab_auth) => {
            info!("Initializing Schwab broker");
            let schwab_config = SchwabConfig {
                auth: schwab_auth.clone(),
                pool: pool.clone(),
            };
            let broker = schwab_config.try_into_broker().await?;
            run_with_broker(config.clone(), pool.clone(), broker).await
        }
        BrokerConfig::Alpaca(alpaca_auth) => {
            info!("Initializing Alpaca broker");
            let broker = alpaca_auth.clone().try_into_broker().await?;
            run_with_broker(config.clone(), pool.clone(), broker).await
        }
    }
}

async fn run_with_broker<B: Broker + Clone>(
    config: Config,
    pool: SqlitePool,
    broker: B,
) -> anyhow::Result<()> {
    if let Some(wait_duration) = broker
        .wait_until_market_open()
        .await
        .map_err(|e| anyhow::anyhow!("Market hours check failed: {}", e))?
    {
        info!(
            "Market is closed, waiting {} minutes until market opens",
            wait_duration.as_secs() / 60
        );
        tokio::time::sleep(wait_duration).await;
    }

    info!("Market is open, starting bot session");

    let (provider, cache, mut clear_stream, mut take_stream) =
        initialize_event_streams(config.clone()).await?;

    let cutoff_block =
        get_cutoff_block(&mut clear_stream, &mut take_stream, &provider, &pool).await?;

    backfill_events(&pool, &provider, &config.evm, cutoff_block - 1).await?;

    conductor::run_live(
        config.clone(),
        pool.clone(),
        cache,
        provider,
        broker,
        clear_stream,
        take_stream,
    )
    .await
}

async fn initialize_event_streams(
    config: Config,
) -> anyhow::Result<(
    impl alloy::providers::Provider + Clone,
    SymbolCache,
    impl Stream<
        Item = Result<
            (bindings::IOrderBookV4::ClearV2, alloy::rpc::types::Log),
            alloy::sol_types::Error,
        >,
    >,
    impl Stream<
        Item = Result<
            (bindings::IOrderBookV4::TakeOrderV2, alloy::rpc::types::Log),
            alloy::sol_types::Error,
        >,
    >,
)> {
    let ws = WsConnect::new(config.evm.ws_rpc_url.as_str());
    let provider = ProviderBuilder::new().connect_ws(ws).await?;
    let cache = SymbolCache::default();
    let orderbook = IOrderBookV4Instance::new(config.evm.orderbook, &provider);

    let clear_stream = orderbook.ClearV2_filter().watch().await?.into_stream();
    let take_stream = orderbook.TakeOrderV2_filter().watch().await?.into_stream();

    Ok((provider, cache, clear_stream, take_stream))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::tests::create_test_config;

    async fn create_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_run_function_websocket_connection_error() {
        let mut config = create_test_config();
        let pool = create_test_pool().await;
        config.evm.ws_rpc_url = "ws://invalid.nonexistent.url:8545".parse().unwrap();
        run(config, pool).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_invalid_orderbook_address() {
        let mut config = create_test_config();
        let pool = create_test_pool().await;
        config.evm.orderbook = alloy::primitives::Address::ZERO;
        config.evm.ws_rpc_url = "ws://localhost:8545".parse().unwrap();
        run(config, pool).await.unwrap_err();
    }

    #[tokio::test]
    async fn test_run_function_error_propagation() {
        let mut config = create_test_config();
        config.database_url = "invalid://database/url".to_string();
        let pool = create_test_pool().await;
        run(config, pool).await.unwrap_err();
    }
}
