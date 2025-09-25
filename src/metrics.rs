use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram},
};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider};
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
    export_interval_secs: u64,
    token_retry_counter: Arc<AtomicU64>,
) -> (SdkMeterProvider, JoinHandle<()>) {
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(export_interval_secs));
        interval.tick().await;

        let get_timestamp = || -> Result<u64, std::time::SystemTimeError> {
            Ok(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos() as u64)
        };

        let service_start_time = get_timestamp().unwrap_or_else(|_| {
            error!("System time is before UNIX epoch, using 0");
            0
        });

        loop {
            interval.tick().await;

            let Ok(now) = get_timestamp() else {
                error!("System time is before UNIX epoch, skipping export");
                continue;
            };

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
                        }]
                    }]
                }]
            });

            let client = reqwest::Client::new();
            match client
                .post(&metrics_endpoint)
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

    let deployment_environment = if env.dry_run { "dev" } else { "prod" };

    info!("Setting up OTLP metrics export to: {endpoint}");

    // Configuration - shorter interval for testing
    let export_interval_secs = 10;
    let metrics_endpoint = format!("{endpoint}/v1/metrics");

    // Create provider for metric collection (without PeriodicReader initially)
    let fallback_provider = SdkMeterProvider::builder()
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

    // Create shared counter for token retries
    let token_retry_counter = Arc::new(AtomicU64::new(0));

    // Use manual HTTP export (OTLP has runtime issues with PeriodicReader)
    info!("Using manual HTTP export to self-hosted Grafana");
    let (provider, flush_task) = create_manual_fallback(
        fallback_provider,
        metrics_endpoint,
        export_interval_secs,
        token_retry_counter.clone(),
    );

    // Now create metrics AFTER the provider is set up
    debug!("Creating metrics using global meter");
    let meter = global::meter("schwarbot");
    let onchain_events_received = meter.u64_counter("onchain_events_received").build();
    let schwab_orders_executed = meter.u64_counter("schwab_orders_executed").build();
    let token_refreshes = meter.u64_counter("token_refreshes").build();
    let queue_depth = meter.u64_gauge("queue_depth").build();
    let accumulated_positions = meter.f64_gauge("accumulated_positions").build();
    let trade_execution_duration_ms = meter.f64_histogram("trade_execution_duration_ms").build();
    let _token_retry_attempts = meter.u64_counter("token_retry_attempts").build();

    // Record startup metric
    let startup_counter = meter.u64_counter("system_startup").build();
    debug!("Recording startup metric");
    startup_counter.add(1, &[KeyValue::new("status", "initialized")]);

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
