use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram},
};
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider};
use std::collections::HashMap;
use tracing::*;

use crate::env::Env;

#[derive(Clone)]
pub(crate) struct Metrics {
    pub(crate) onchain_events_received: Counter<u64>,
    pub(crate) schwab_orders_executed: Counter<u64>,
    pub(crate) token_refreshes: Counter<u64>,
    pub(crate) queue_depth: Gauge<u64>,
    pub(crate) accumulated_positions: Gauge<f64>,
    pub(crate) trade_execution_duration_ms: Histogram<f64>,
    pub(crate) provider: SdkMeterProvider,
}

pub(crate) fn setup(env: &Env) -> Option<Metrics> {
    let endpoint = env.otel_metrics_exporter_endpoint.as_ref()?;
    let auth_token = env.otel_metrics_exporter_basic_auth_token.as_ref()?;

    debug!("Setting up metrics with endpoint: {}", endpoint);

    let deployment_environment = if env.dry_run { "dev" } else { "prod" };

    let exporter = match opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_protocol(Protocol::HttpBinary)
        .with_headers(HashMap::from([(
            "Authorization".to_string(),
            format!("Basic {}", auth_token),
        )]))
        .build()
    {
        Ok(exporter) => exporter,
        Err(e) => {
            error!("Failed to build metrics exporter: {}", e);
            return None;
        }
    };

    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("schwarbot")
                .with_attributes(vec![KeyValue::new(
                    "deployment.environment",
                    deployment_environment,
                )])
                .build(),
        )
        .build();

    global::set_meter_provider(provider.clone());
    let meter = global::meter("schwarbot");

    let onchain_events_received = meter.u64_counter("onchain_events_received").build();
    let schwab_orders_executed = meter.u64_counter("schwab_orders_executed").build();
    let token_refreshes = meter.u64_counter("token_refreshes").build();
    let queue_depth = meter.u64_gauge("queue_depth").build();
    let accumulated_positions = meter.f64_gauge("accumulated_positions").build();
    let trade_execution_duration_ms = meter.f64_histogram("trade_execution_duration_ms").build();

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
        // Shutdown is handled automatically by the SDK when the provider is dropped
    }
}
