use clap::Parser;
use sqlx::SqlitePool;
use tracing::Level;

use crate::offchain::order_poller::OrderPollerConfig;
use crate::onchain::EvmEnv;
use st0x_broker::alpaca::auth::AlpacaAuthEnv;
use st0x_broker::schwab::auth::SchwabAuthEnv;
use st0x_broker::{AlpacaBroker, Broker, SchwabBroker, SupportedBroker, TestBroker};

#[derive(clap::ValueEnum, Debug, Clone)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<LogLevel> for Level {
    fn from(log_level: LogLevel) -> Self {
        match log_level {
            LogLevel::Trace => Self::TRACE,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Info => Self::INFO,
            LogLevel::Warn => Self::WARN,
            LogLevel::Error => Self::ERROR,
        }
    }
}

impl From<&LogLevel> for Level {
    fn from(log_level: &LogLevel) -> Self {
        match log_level {
            LogLevel::Trace => Self::TRACE,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Info => Self::INFO,
            LogLevel::Warn => Self::WARN,
            LogLevel::Error => Self::ERROR,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub struct Env {
    #[clap(long = "db", env)]
    pub database_url: String,
    #[clap(long, env, default_value = "debug")]
    pub log_level: LogLevel,
    #[clap(long, env, default_value = "8080")]
    pub server_port: u16,
    #[clap(flatten)]
    pub schwab_auth: SchwabAuthEnv,
    #[clap(flatten)]
    pub alpaca_auth: AlpacaAuthEnv,
    #[clap(flatten)]
    pub evm_env: EvmEnv,
    /// Interval in seconds between order status polling checks
    #[clap(long, env, default_value = "15")]
    pub order_polling_interval: u64,
    /// Maximum jitter in seconds for order polling to prevent thundering herd
    #[clap(long, env, default_value = "5")]
    pub order_polling_max_jitter: u64,
    /// Broker to use for trading (required: schwab, alpaca, or dry-run)
    #[clap(long, env)]
    pub broker: SupportedBroker,
}

impl Env {
    pub async fn get_sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        SqlitePool::connect(&self.database_url).await
    }

    pub const fn get_order_poller_config(&self) -> OrderPollerConfig {
        OrderPollerConfig {
            polling_interval: std::time::Duration::from_secs(self.order_polling_interval),
            max_jitter: std::time::Duration::from_secs(self.order_polling_max_jitter),
        }
    }

    pub(crate) async fn get_schwab_broker(
        &self,
        pool: SqlitePool,
    ) -> Result<SchwabBroker, <SchwabBroker as Broker>::Error> {
        SchwabBroker::try_from_config((self.schwab_auth.clone(), pool)).await
    }

    pub(crate) async fn get_alpaca_broker(
        &self,
    ) -> Result<AlpacaBroker, <AlpacaBroker as Broker>::Error> {
        AlpacaBroker::try_from_config(self.alpaca_auth.clone()).await
    }

    pub(crate) async fn get_test_broker(
        &self,
    ) -> Result<TestBroker, <TestBroker as Broker>::Error> {
        TestBroker::try_from_config(()).await
    }
}

pub fn setup_tracing(log_level: &LogLevel) {
    let level: Level = log_level.into();
    let default_filter = format!("st0x_hedge={level},auth={level},main={level}");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::onchain::EvmEnv;
    use alloy::primitives::address;
    use st0x_broker::schwab::auth::SchwabAuthEnv;

    pub fn create_test_env_with_order_owner(order_owner: alloy::primitives::Address) -> Env {
        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            server_port: 8080,
            schwab_auth: SchwabAuthEnv {
                schwab_app_key: "test_key".to_string(),
                schwab_app_secret: "test_secret".to_string(),
                schwab_redirect_uri: "https://127.0.0.1".to_string(),
                schwab_base_url: "https://test.com".to_string(),
                schwab_account_index: 0,
            },
            alpaca_auth: AlpacaAuthEnv {
                alpaca_api_key_id: String::new(),
                alpaca_api_secret_key: String::new(),
                alpaca_base_url: "https://paper-api.alpaca.markets".to_string(),
            },
            evm_env: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1111111111111111111111111111111111111111"),
                order_owner,
                deployment_block: 1,
            },
            order_polling_interval: 15,
            order_polling_max_jitter: 5,
            broker: SupportedBroker::Schwab,
        }
    }

    pub fn create_test_env() -> Env {
        create_test_env_with_order_owner(address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
    }

    #[test]
    fn test_log_level_from_conversion() {
        let level: Level = LogLevel::Trace.into();
        assert_eq!(Level::TRACE, level);

        let level: Level = LogLevel::Debug.into();
        assert_eq!(Level::DEBUG, level);

        let level: Level = LogLevel::Info.into();
        assert_eq!(Level::INFO, level);

        let level: Level = LogLevel::Warn.into();
        assert_eq!(Level::WARN, level);

        let level: Level = LogLevel::Error.into();
        assert_eq!(Level::ERROR, level);

        // Test reference conversion
        let log_level = LogLevel::Debug;
        let level: Level = (&log_level).into();
        assert_eq!(level, Level::DEBUG);
    }

    #[tokio::test]
    async fn test_env_sqlite_pool_creation() {
        let env = create_test_env();
        let pool_result = env.get_sqlite_pool().await;
        assert!(pool_result.is_ok());
    }

    #[tokio::test]
    async fn test_get_broker_types() {
        let env = create_test_env();
        let pool = crate::test_utils::setup_test_db().await;

        // SchwabBroker creation should fail without valid tokens
        let schwab_result = env.get_schwab_broker(pool.clone()).await;
        assert!(schwab_result.is_err());

        // TestBroker should always work
        let test_broker = env.get_test_broker().await.unwrap();
        assert!(format!("{test_broker:?}").contains("TestBroker"));
    }

    #[test]
    fn test_env_construction() {
        let env = create_test_env();
        assert_eq!(env.database_url, ":memory:");
        assert!(matches!(env.log_level, LogLevel::Debug));
        assert_eq!(env.schwab_auth.schwab_app_key, "test_key");
        assert_eq!(env.evm_env.deployment_block, 1);
    }
}
