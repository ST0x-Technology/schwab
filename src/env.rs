use clap::Parser;
use sqlx::SqlitePool;
use tracing::Level;

use crate::onchain::EvmEnv;
use crate::schwab::SchwabAuthEnv;

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
    pub evm_env: EvmEnv,
    /// Interval in seconds between order status polling checks
    #[clap(long, env, default_value = "15")]
    pub order_polling_interval: u64,
    /// Maximum jitter in seconds for order polling to prevent thundering herd
    #[clap(long, env, default_value = "5")]
    pub order_polling_max_jitter: u64,
}

impl Env {
    pub async fn get_sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        SqlitePool::connect(&self.database_url).await
    }

    pub const fn get_order_poller_config(&self) -> crate::schwab::OrderPollerConfig {
        crate::schwab::OrderPollerConfig {
            polling_interval: std::time::Duration::from_secs(self.order_polling_interval),
            max_jitter: std::time::Duration::from_secs(self.order_polling_max_jitter),
        }
    }
}

pub fn setup_tracing(log_level: &LogLevel) {
    let level: Level = log_level.into();
    let default_filter = format!("rain_schwab={level},auth={level},main={level}");

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
    use crate::schwab::SchwabAuthEnv;
    use alloy::primitives::{address, fixed_bytes};

    pub fn create_test_env_with_order_hash(order_hash: alloy::primitives::B256) -> Env {
        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            server_port: 8080,
            schwab_auth: SchwabAuthEnv {
                app_key: "test_key".to_string(),
                app_secret: "test_secret".to_string(),
                redirect_uri: "https://127.0.0.1".to_string(),
                base_url: "https://test.com".to_string(),
                account_index: 0,
            },
            evm_env: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1111111111111111111111111111111111111111"),
                order_hash,
                deployment_block: 1,
            },
            order_polling_interval: 15,
            order_polling_max_jitter: 5,
        }
    }

    pub fn create_test_env() -> Env {
        create_test_env_with_order_hash(fixed_bytes!(
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ))
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

    #[test]
    fn test_env_construction() {
        let env = create_test_env();
        assert_eq!(env.database_url, ":memory:");
        assert!(matches!(env.log_level, LogLevel::Debug));
        assert_eq!(env.schwab_auth.app_key, "test_key");
        assert_eq!(env.evm_env.deployment_block, 1);
    }
}
