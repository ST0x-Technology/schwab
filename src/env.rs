use clap::Parser;
use opentelemetry::trace::TracerProvider;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor};
use opentelemetry_sdk::{Resource, trace as sdktrace};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tracing::{Level, error, warn};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

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
    /// OpenTelemetry exporter endpoint (defaults to HyperDX if not specified)
    #[clap(long, env)]
    pub otel_exporter_endpoint: Option<String>,
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

pub fn setup_tracing(env: &Env) -> Option<Arc<sdktrace::SdkTracerProvider>> {
    let level: Level = (&env.log_level).into();
    let default_filter = format!("rain_schwab={level},auth={level},main={level}");

    if let Some(ref api_key) = env.hyperdx_api_key {
        setup_hyperdx_tracing(&default_filter, api_key, env)
    } else if env.otel_exporter_endpoint.is_some() {
        setup_custom_endpoint_tracing(&default_filter, env)
    } else {
        setup_console_tracing(&default_filter);
        None
    }
}

fn setup_hyperdx_tracing(
    default_filter: &str,
    api_key: &str,
    env: &Env,
) -> Option<Arc<sdktrace::SdkTracerProvider>> {
    match setup_tracing_with_hyperdx(
        default_filter,
        api_key,
        env.hyperdx_service_name.clone(),
        env.otel_exporter_endpoint.as_deref(),
    ) {
        Ok(provider) => Some(provider),
        Err(e) => {
            error!("Failed to setup HyperDX tracing: {e}");
            None
        }
    }
}

fn setup_custom_endpoint_tracing(
    default_filter: &str,
    env: &Env,
) -> Option<Arc<sdktrace::SdkTracerProvider>> {
    match setup_tracing_with_hyperdx(
        default_filter,
        "dummy", // No API key needed for custom endpoints
        env.hyperdx_service_name.clone(),
        env.otel_exporter_endpoint.as_deref(),
    ) {
        Ok(provider) => Some(provider),
        Err(e) => {
            error!("Failed to setup custom endpoint tracing: {e}");
            None
        }
    }
}

fn setup_console_tracing(default_filter: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .compact()
        .init();

    warn!("No HYPERDX_API_KEY configured - running with console logging only");
}

fn setup_tracing_with_hyperdx(
    default_filter: &str,
    api_key: &str,
    service_name: String,
    custom_endpoint: Option<&str>,
) -> Result<Arc<sdktrace::SdkTracerProvider>, Box<dyn std::error::Error + Send + Sync>> {
    // 1. Build resource (following gist pattern exactly)
    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_attributes(vec![KeyValue::new("deployment.environment", "production")])
        .build();

    // 2. Create OTLP exporter
    const HYPERDX_ENDPOINT: &str = "https://in-otel.hyperdx.io/v1/traces";
    let endpoint = custom_endpoint.unwrap_or(HYPERDX_ENDPOINT);
    let is_hyperdx = custom_endpoint.is_none();

    let mut headers = std::collections::HashMap::new();
    if is_hyperdx {
        headers.insert("authorization".to_string(), api_key.to_string());
    }

    let http_client = std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .gzip(true)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
    .join()
    .map_err(|_| "Failed to create HTTP client in background thread")?;

    let otlp_exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_headers(headers)
        .with_http_client(http_client)
        .with_protocol(if is_hyperdx {
            Protocol::Grpc
        } else {
            Protocol::HttpJson
        })
        .build()
        .map_err(|e| {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| default_filter.into()),
                )
                .compact()
                .init();
            error!("Failed to create OTLP exporter: {e}, falling back to console logging");
            format!("Failed to create OTLP exporter: {e}")
        })?;

    // 3. Build tracer provider with batch processor
    let batch_exporter = BatchSpanProcessor::builder(otlp_exporter)
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_max_export_batch_size(512)
                .with_max_queue_size(2048)
                .with_scheduled_delay(Duration::from_secs(3))
                .build(),
        )
        .build();

    let tracer_provider = sdktrace::SdkTracerProvider::builder()
        .with_span_processor(batch_exporter)
        .with_resource(resource)
        .build();

    // 4. Set global tracer provider (needed for explicit OpenTelemetry spans)
    global::set_tracer_provider(tracer_provider.clone());

    // 5. Set text map propagator
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    // 6. Get tracer from provider
    let tracer = tracer_provider.tracer("schwab-bot");

    // 7. Create tracing layer with the tracer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    // 8. Create subscriber with Registry exactly like gist
    let subscriber = Registry::default().with(telemetry);

    // 9. Set as global default
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| format!("Failed to set global subscriber: {e}"))?;

    Ok(Arc::new(tracer_provider))
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
            otel_exporter_endpoint: None,
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
