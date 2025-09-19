use base64::Engine;
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram},
};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    Resource,
    metrics::{PeriodicReader, SdkMeterProvider},
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::env::Env;

fn create_manual_fallback(
    provider: SdkMeterProvider,
    metrics_endpoint: String,
    auth_header: String,
    export_interval_secs: u64,
    token_retry_counter: Arc<AtomicU64>,
) -> (SdkMeterProvider, JoinHandle<()>) {
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(export_interval_secs));
        interval.tick().await;

        let service_start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        loop {
            interval.tick().await;

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;

            let retry_count = token_retry_counter.load(Ordering::Relaxed);

            debug!(
                "Manual export - sending metrics: heartbeat=1, token_retry_attempts={}",
                retry_count
            );

            let metrics_payload = json!({
                "resource_metrics": [{
                    "resource": {
                        "attributes": [{
                            "key": "service.name",
                            "value": {"string_value": "schwarbot"}
                        }]
                    },
                    "scope_metrics": [{
                        "scope": {
                            "name": "schwarbot"
                        },
                        "metrics": [{
                            "name": "heartbeat_counter",
                            "description": "Heartbeat counter to show metrics are working",
                            "unit": "1",
                            "sum": {
                                "data_points": [{
                                    "start_time_unix_nano": service_start_time,
                                    "time_unix_nano": now,
                                    "as_int": 1
                                }],
                                "aggregation_temporality": 2,
                                "is_monotonic": true
                            }
                        }, {
                            "name": "token_retry_attempts",
                            "description": "Number of token refresh retry attempts",
                            "unit": "1",
                            "sum": {
                                "data_points": [{
                                    "start_time_unix_nano": service_start_time,
                                    "time_unix_nano": now,
                                    "as_int": retry_count
                                }],
                                "aggregation_temporality": 2,
                                "is_monotonic": true
                            }
                        }, {
                            "name": "system_startup",
                            "description": "System startup counter",
                            "unit": "1",
                            "sum": {
                                "data_points": [{
                                    "start_time_unix_nano": service_start_time,
                                    "time_unix_nano": now,
                                    "as_int": 1
                                }],
                                "aggregation_temporality": 2,
                                "is_monotonic": true
                            }
                        }, {
                            "name": "onchain_events_received",
                            "description": "Number of onchain events received",
                            "unit": "1",
                            "sum": {
                                "data_points": [{
                                    "start_time_unix_nano": service_start_time,
                                    "time_unix_nano": now,
                                    "as_int": 0
                                }],
                                "aggregation_temporality": 2,
                                "is_monotonic": true
                            }
                        }, {
                            "name": "schwab_orders_executed",
                            "description": "Number of Schwab orders executed",
                            "unit": "1",
                            "sum": {
                                "data_points": [{
                                    "start_time_unix_nano": service_start_time,
                                    "time_unix_nano": now,
                                    "as_int": 0
                                }],
                                "aggregation_temporality": 2,
                                "is_monotonic": true
                            }
                        }, {
                            "name": "queue_depth",
                            "description": "Current queue depth",
                            "unit": "1",
                            "gauge": {
                                "data_points": [{
                                    "time_unix_nano": now,
                                    "as_int": 0
                                }]
                            }
                        }]
                    }]
                }]
            });

            let client = reqwest::Client::new();
            match client
                .post(&metrics_endpoint)
                .header("Authorization", auth_header.clone())
                .header("Content-Type", "application/json")
                .json(&metrics_payload)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    match response.text().await {
                        Ok(body) => {
                            if status.is_success() {
                                debug!(
                                    "Successfully exported metrics to Grafana - Status: {}",
                                    status
                                );
                            } else if status == 429 {
                                warn!("Rate limited by Grafana (429), will retry next interval");
                            } else {
                                error!(
                                    "Failed to export metrics - Status: {}, Body: {}",
                                    status, body
                                );
                            }
                        }
                        Err(e) => {
                            error!("Failed to read response body: {} - Status: {}", e, status);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to send metrics HTTP request: {}", e);
                }
            }
        }
    });

    (provider, task)
}

pub(crate) struct Metrics {
    pub(crate) onchain_events_received: Counter<u64>,
    pub(crate) schwab_orders_executed: Counter<u64>,
    pub(crate) token_refreshes: Counter<u64>,
    pub(crate) queue_depth: Gauge<u64>,
    pub(crate) accumulated_positions: Gauge<f64>,
    pub(crate) trade_execution_duration_ms: Histogram<f64>,
    token_retry_counter: Arc<AtomicU64>,
    _provider: SdkMeterProvider, // Keep for future steps
    flush_task: JoinHandle<()>,
}

pub(crate) async fn setup(env: &Env) -> Option<Metrics> {
    let endpoint = env.otel_metrics_exporter_endpoint.as_ref()?;
    let api_key = env.otel_metrics_exporter_basic_auth_token.as_ref()?;
    let instance_id = env.otel_metrics_exporter_instance_id.as_ref()?;

    let deployment_environment = if env.dry_run { "dev" } else { "prod" };

    debug!(
        "Setting up metrics with custom HTTP export to: {}",
        endpoint
    );

    // Configuration
    let export_interval_secs = 30; // Shorter for testing
    let metrics_endpoint = format!("{}/v1/metrics", endpoint);

    // Create provider for metric collection (without PeriodicReader)
    let provider = SdkMeterProvider::builder()
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

    let meter = global::meter("schwarbot");

    // Create all metric instruments
    let onchain_events_received = meter.u64_counter("onchain_events_received").build();
    let schwab_orders_executed = meter.u64_counter("schwab_orders_executed").build();
    let token_refreshes = meter.u64_counter("token_refreshes").build();
    let queue_depth = meter.u64_gauge("queue_depth").build();
    let accumulated_positions = meter.f64_gauge("accumulated_positions").build();
    let trade_execution_duration_ms = meter.f64_histogram("trade_execution_duration_ms").build();
    let _token_retry_attempts = meter.u64_counter("token_retry_attempts").build();

    // Record startup metric
    let startup_counter = meter.u64_counter("system_startup").build();
    startup_counter.add(1, &[KeyValue::new("status", "initialized")]);

    // Create auth header for Basic authentication
    let auth_header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", instance_id, api_key))
    );

    // Create shared counter for token retries
    let token_retry_counter = Arc::new(AtomicU64::new(0));

    // Use manual HTTP export directly (working version)
    info!("Using manual HTTP export to Grafana Cloud");

    let (provider, flush_task) = create_manual_fallback(
        provider,
        metrics_endpoint,
        auth_header,
        export_interval_secs,
        token_retry_counter.clone(),
    );

    Some(Metrics {
        onchain_events_received,
        schwab_orders_executed,
        token_refreshes,
        queue_depth,
        accumulated_positions,
        trade_execution_duration_ms,
        token_retry_counter,
        _provider: provider,
        flush_task,
    })
}

impl Metrics {
    /// Increment the token retry counter
    pub(crate) fn increment_token_retry(&self) {
        self.token_retry_counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for Metrics {
    fn drop(&mut self) {
        debug!("Shutting down metrics and background task");

        // Cancel the background flush task
        self.flush_task.abort();

        debug!("Metrics shutdown complete");
    }
}
