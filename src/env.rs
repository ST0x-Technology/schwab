use clap::Parser;
use opentelemetry::trace::TracerProvider;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{Resource, trace as sdktrace};
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::onchain::EvmEnv;
use crate::schwab::OrderPollerConfig;
use crate::schwab::SchwabAuthEnv;
use crate::schwab::broker::{DynBroker, LogBroker, Schwab};

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
    #[clap(long, env, default_value = "false")]
    pub dry_run: bool,
    /// HyperDX API key for telemetry export (optional)
    #[clap(long, env)]
    pub hyperdx_api_key: Option<String>,
    /// Service name for HyperDX identification
    #[clap(long, env, default_value = "schwab-bot")]
    pub hyperdx_service_name: String,
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

    pub(crate) fn get_broker(&self) -> DynBroker {
        if self.dry_run {
            Arc::new(LogBroker::new())
        } else {
            Arc::new(Schwab)
        }
    }
}

pub fn setup_tracing(env: &Env) {
    let level: Level = (&env.log_level).into();
    let default_filter = format!("rain_schwab={level},auth={level},main={level}");

    if let Some(ref api_key) = env.hyperdx_api_key {
        // Set up OpenTelemetry with HyperDX
        setup_tracing_with_hyperdx(
            default_filter,
            api_key.clone(),
            env.hyperdx_service_name.clone(),
        );
    } else {
        // Console logging only
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| default_filter.into()),
            )
            .compact()
            .init();

        // Warn user about console-only mode
        tracing::warn!("No HYPERDX_API_KEY configured - running with console logging only");
    }
}

fn setup_tracing_with_hyperdx(default_filter: String, api_key: String, service_name: String) {
    // Create resource with service information
    let resource = Resource::builder()
        .with_attributes(vec![
            KeyValue::new("service.name", service_name),
            KeyValue::new("deployment.environment", "production"),
        ])
        .build();

    // Configure OTLP exporter for HyperDX
    let mut headers = std::collections::HashMap::new();
    headers.insert("authorization".to_string(), format!("Bearer {}", api_key));

    println!("Setting up HyperDX OTLP exporter:");
    println!("  Endpoint: https://in-otel.hyperdx.io/v1/traces");
    println!("  Service: {}", service_name);
    println!(
        "  API Key: {}...{}",
        &api_key[..4.min(api_key.len())],
        &api_key[api_key.len().saturating_sub(4)..]
    );

    let otlp_exporter = match SpanExporter::builder()
        .with_http()
        .with_endpoint("https://in-otel.hyperdx.io/v1/traces")
        .with_headers(headers)
        .with_http_client(reqwest::Client::new())
        .build()
    {
        Ok(exporter) => exporter,
        Err(e) => {
            eprintln!("Failed to create OTLP exporter: {e}, falling back to console logging");
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| default_filter.into()),
                )
                .compact()
                .init();
            return;
        }
    };

    // Create tracer provider with batch exporter
    let tracer_provider = sdktrace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        .build();

    // Set as global tracer provider
    global::set_tracer_provider(tracer_provider.clone());

    // Get tracer and create OpenTelemetry layer
    let tracer = tracer_provider.tracer("schwab-bot");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    println!("✅ HyperDX OTLP exporter configured successfully");
    println!("📡 Telemetry will be sent to: https://in-otel.hyperdx.io/v1/traces");

    // Create console layer
    let fmt_layer = tracing_subscriber::fmt::layer().compact();

    // Combine layers and initialize subscriber
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    println!("🔍 Tracing initialized with both console and HyperDX layers");
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::onchain::EvmEnv;
    use crate::schwab::SchwabAuthEnv;
    use alloy::primitives::address;

    pub fn create_test_env_with_order_owner(order_owner: alloy::primitives::Address) -> Env {
        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
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
                order_owner,
                deployment_block: 1,
            },
            order_polling_interval: 15,
            order_polling_max_jitter: 5,
            dry_run: false,
            hyperdx_api_key: None,
            hyperdx_service_name: "schwab-bot".to_string(),
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

    #[test]
    fn test_get_broker_dry_run_modes() {
        // Test dry_run = false (should return Schwab broker)
        let mut env = create_test_env();
        env.dry_run = false;
        let broker = env.get_broker();
        assert_eq!(format!("{broker:?}"), "Schwab");

        // Test dry_run = true (should return LogBroker)
        env.dry_run = true;
        let broker = env.get_broker();
        assert!(format!("{broker:?}").contains("LogBroker"));
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
