use clap::Parser;
use sqlx::SqlitePool;
use tracing::Level;

use crate::offchain::order_poller::OrderPollerConfig;
use crate::onchain::EvmEnv;
use st0x_broker::SupportedBroker;
use st0x_broker::alpaca::auth::AlpacaAuthEnv;
use st0x_broker::schwab::auth::SchwabAuthEnv;

#[derive(Debug, Clone)]
pub enum BrokerConfig {
    Schwab(SchwabAuthEnv),
    Alpaca(AlpacaAuthEnv),
    DryRun,
}

impl BrokerConfig {
    pub fn to_supported_broker(&self) -> SupportedBroker {
        match self {
            Self::Schwab(_) => SupportedBroker::Schwab,
            Self::Alpaca(_) => SupportedBroker::Alpaca,
            Self::DryRun => SupportedBroker::DryRun,
        }
    }
}

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

#[derive(Debug, Clone)]
pub struct Config {
    pub(crate) database_url: String,
    pub log_level: LogLevel,
    pub(crate) server_port: u16,
    pub(crate) evm: EvmEnv,
    pub(crate) order_polling_interval: u64,
    pub(crate) order_polling_max_jitter: u64,
    pub(crate) broker: BrokerConfig,
}

#[derive(Parser, Debug, Clone)]
pub struct Env {
    #[clap(long = "db", env)]
    database_url: String,
    #[clap(long, env, default_value = "debug")]
    log_level: LogLevel,
    #[clap(long, env, default_value = "8080")]
    server_port: u16,
    #[clap(flatten)]
    schwab_auth: SchwabAuthEnv,
    #[clap(flatten)]
    alpaca_auth: AlpacaAuthEnv,
    #[clap(flatten)]
    pub(crate) evm: EvmEnv,
    /// Interval in seconds between order status polling checks
    #[clap(long, env, default_value = "15")]
    order_polling_interval: u64,
    /// Maximum jitter in seconds for order polling to prevent thundering herd
    #[clap(long, env, default_value = "5")]
    order_polling_max_jitter: u64,
    /// Broker to use for trading (required: schwab, alpaca, or dry-run)
    #[clap(long, env)]
    broker: SupportedBroker,
}

impl Env {
    pub fn into_config(self) -> Config {
        let broker = match self.broker {
            SupportedBroker::Schwab => BrokerConfig::Schwab(self.schwab_auth),
            SupportedBroker::Alpaca => BrokerConfig::Alpaca(self.alpaca_auth),
            SupportedBroker::DryRun => BrokerConfig::DryRun,
        };

        Config {
            database_url: self.database_url,
            log_level: self.log_level,
            server_port: self.server_port,
            evm: self.evm,
            order_polling_interval: self.order_polling_interval,
            order_polling_max_jitter: self.order_polling_max_jitter,
            broker,
        }
    }
}

impl Config {
    pub async fn get_sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        SqlitePool::connect(&self.database_url).await
    }

    pub const fn get_order_poller_config(&self) -> OrderPollerConfig {
        OrderPollerConfig {
            polling_interval: std::time::Duration::from_secs(self.order_polling_interval),
            max_jitter: std::time::Duration::from_secs(self.order_polling_max_jitter),
        }
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
    use st0x_broker::{MockBrokerConfig, TryIntoBroker};

    pub fn create_test_config_with_order_owner(order_owner: alloy::primitives::Address) -> Config {
        Config {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            server_port: 8080,
            evm: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1111111111111111111111111111111111111111"),
                order_owner,
                deployment_block: 1,
            },
            order_polling_interval: 15,
            order_polling_max_jitter: 5,
            broker: BrokerConfig::Schwab(SchwabAuthEnv {
                schwab_app_key: "test_key".to_string(),
                schwab_app_secret: "test_secret".to_string(),
                schwab_redirect_uri: "https://127.0.0.1".to_string(),
                schwab_base_url: "https://test.com".to_string(),
                schwab_account_index: 0,
            }),
        }
    }

    pub fn create_test_config() -> Config {
        create_test_config_with_order_owner(address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"))
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
    async fn test_config_sqlite_pool_creation() {
        let config = create_test_config();
        let pool_result = config.get_sqlite_pool().await;
        assert!(pool_result.is_ok());
    }

    #[tokio::test]
    async fn test_get_broker_types() {
        let config = create_test_config();
        let pool = crate::test_utils::setup_test_db().await;

        // SchwabBroker creation should fail without valid tokens
        let BrokerConfig::Schwab(schwab_auth) = &config.broker else {
            panic!("Expected Schwab broker config");
        };
        let schwab_config = st0x_broker::schwab::broker::SchwabConfig {
            auth: schwab_auth.clone(),
            pool: pool.clone(),
        };
        let schwab_result = schwab_config.try_into_broker().await;
        assert!(schwab_result.is_err());

        // MockBroker should always work
        let test_broker = MockBrokerConfig.try_into_broker().await.unwrap();
        assert!(format!("{test_broker:?}").contains("MockBroker"));
    }

    #[test]
    fn test_config_construction() {
        let config = create_test_config();
        assert_eq!(config.database_url, ":memory:");
        assert!(matches!(config.log_level, LogLevel::Debug));
        assert!(matches!(config.broker, BrokerConfig::Schwab(_)));
        assert_eq!(config.evm.deployment_block, 1);
    }
}
