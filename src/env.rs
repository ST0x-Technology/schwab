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

#[derive(Debug)]
struct DebugLayer;

impl<S> tracing_subscriber::Layer<S> for DebugLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let span = ctx.span(id).expect("span should exist");
        println!(
            "📍 New span: {} (id: {:?}, level: {:?})",
            attrs.metadata().name(),
            id,
            attrs.metadata().level()
        );
        // Check if span has OpenTelemetry extensions
        if span
            .extensions()
            .get::<tracing_opentelemetry::OtelData>()
            .is_some()
        {
            println!("   ✅ Has OpenTelemetry data");
        } else {
            println!("   ❌ Missing OpenTelemetry data");
        }
    }

    fn on_enter(&self, id: &tracing::span::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        println!("➡️ Entering span: {id:?}");
    }

    fn on_exit(&self, id: &tracing::span::Id, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        println!("⬅️ Exiting span: {id:?}");
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

pub fn setup_tracing(env: &Env) {
    let level: Level = (&env.log_level).into();
    // Include OpenTelemetry internal logging for debugging
    let default_filter = format!(
        "rain_schwab={level},auth={level},main={level},opentelemetry={level},opentelemetry_otlp={level},opentelemetry_sdk={level}"
    );

    if let Some(ref api_key) = env.hyperdx_api_key {
        // Set up OpenTelemetry with HyperDX
        setup_tracing_with_hyperdx(
            default_filter,
            api_key,
            env.hyperdx_service_name.clone(),
            env.otel_exporter_endpoint.as_deref(),
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

fn setup_tracing_with_hyperdx(
    default_filter: String,
    api_key: &str,
    service_name: String,
    custom_endpoint: Option<&str>,
) {
    const HYPERDX_ENDPOINT: &str = "https://in-otel.hyperdx.io/v1/traces";

    let endpoint = custom_endpoint.unwrap_or(HYPERDX_ENDPOINT);
    let is_hyperdx = custom_endpoint.is_none();

    println!("Setting up OTLP exporter:");
    println!("  Endpoint: {endpoint}");
    println!("  Service: {service_name}");
    if is_hyperdx {
        println!("  Mode: HyperDX (with API key authentication)");
    } else {
        println!("  Mode: Custom endpoint (no authentication)");
    }

    // Create resource with service information
    let resource = Resource::builder()
        .with_attributes(vec![
            KeyValue::new("service.name", service_name),
            KeyValue::new("deployment.environment", "production"),
        ])
        .build();

    let mut headers = std::collections::HashMap::new();
    if is_hyperdx {
        headers.insert("authorization".to_string(), format!("Bearer {api_key}"));
        println!(
            "  API Key: {}...{}",
            &api_key[..4.min(api_key.len())],
            &api_key[api_key.len().saturating_sub(4)..]
        );
    }

    let otlp_exporter = match SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
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
            panic!("Failed to create OTLP exporter: {e}");
        }
    };

    // Create tracer provider with simple exporter to avoid runtime issues
    let tracer_provider = sdktrace::SdkTracerProvider::builder()
        .with_simple_exporter(otlp_exporter)
        .with_resource(resource)
        .build();

    // Set as global tracer provider
    global::set_tracer_provider(tracer_provider.clone());

    // Set up global propagator (CRITICAL for span propagation)
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    // TODO: Add log export once we resolve the Tokio runtime context issue

    // Get tracer and create OpenTelemetry layer
    let tracer = tracer_provider.tracer("schwab-bot");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    println!("✅ OTLP exporter configured successfully");
    println!("📡 Traces: {endpoint}");

    // Create console layer
    let fmt_layer = tracing_subscriber::fmt::layer().compact();

    // Combine layers and initialize subscriber (OpenTelemetry layer first)
    tracing_subscriber::registry()
        .with(otel_layer) // OpenTelemetry first
        .with(DebugLayer) // Debug layer AFTER OpenTelemetry to see its data
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .with(fmt_layer) // Console output last
        .init();

    println!("🔍 Tracing initialized with both console and HyperDX layers");
    println!("⏱️  Simple exporter will export spans immediately");

    // Skip OpenTelemetry shutdown to avoid "there is no reactor running" panic
    // The batch exporter will handle its own cleanup when the process exits
    println!("⚠️  OpenTelemetry shutdown skipped to avoid runtime issues");
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
