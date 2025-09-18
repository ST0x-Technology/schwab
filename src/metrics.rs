use opentelemetry::metrics::{Counter, Gauge, Histogram};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{metrics::reader::DefaultTemporalitySelector, Resource};
use std::{collections::HashMap, time::Duration};
use tracing::*;

use crate::env::Env;

pub type MeterProvider = std::sync::Arc<dyn opentelemetry::metrics::MeterProvider + Send + Sync>;

#[derive(Clone)]
pub struct Metrics {
    pub onchain_events_received: Counter<u64>,
    pub schwab_orders_executed: Counter<u64>,
    pub token_refreshes: Counter<u64>,
    pub queue_depth: Gauge<u64>,
    pub accumulated_positions: Gauge<f64>,
    pub trade_execution_duration_ms: Histogram<f64>,
    pub provider: MeterProvider,
}

pub fn setup(env: &Env) -> Option<Metrics> {
    let endpoint = env.otel_metrics_exporter_endpoint.as_ref()?;
    let auth_token = env.otel_metrics_exporter_basic_auth_token.as_ref()?;
    
    debug!("Setting up metrics with endpoint: {}", endpoint);

    let deployment_environment = if env.dry_run { "dev" } else { "prod" };

    let provider = match opentelemetry_otlp::new_pipeline()
        .metrics(opentelemetry_sdk::runtime::Tokio)
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_headers(HashMap::from([(
                    "Authorization".to_string(),
                    format!("Basic {}", auth_token),
                )])),
        )
        .with_resource(Resource::new(vec![
            opentelemetry::KeyValue::new("service.name", "schwarbot"),
            opentelemetry::KeyValue::new("deployment.environment", deployment_environment),
        ]))
        .with_period(Duration::from_secs(3))
        .with_timeout(Duration::from_secs(10))
        .with_temporality_selector(DefaultTemporalitySelector::new())
        .build()
    {
        Ok(provider) => provider,
        Err(e) => {
            error!("Failed to setup metrics provider: {}", e);
            return None;
        }
    };

    debug!("{provider:#?}");

    opentelemetry::global::set_meter_provider(provider);
    let provider = opentelemetry::global::meter_provider();

    let meter = provider.meter("schwarbot");
    
    let onchain_events_received = meter.u64_counter("onchain_events_received").init();
    let schwab_orders_executed = meter.u64_counter("schwab_orders_executed").init();
    let token_refreshes = meter.u64_counter("token_refreshes").init();
    let queue_depth = meter.u64_gauge("queue_depth").init();
    let accumulated_positions = meter.f64_gauge("accumulated_positions").init();
    let trade_execution_duration_ms = meter
        .f64_histogram("trade_execution_duration_ms")
        .init();

    info!("Successfully set up OTLP metrics");

    Some(Metrics {
        onchain_events_received,
        schwab_orders_executed,
        token_refreshes,
        queue_depth,
        accumulated_positions,
        trade_execution_duration_ms,
        provider,
    })
}

impl Drop for Metrics {
    fn drop(&mut self) {
        debug!("Shutting down metrics provider");
        if let Err(e) = opentelemetry::global::shutdown_meter_provider() {
            error!("Failed to shutdown metrics provider: {}", e);
        }
    }
}