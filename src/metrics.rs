use base64::{Engine as _, engine::general_purpose};
use opentelemetry::{
    KeyValue, global,
    metrics::{Counter, Gauge, Histogram},
};
use opentelemetry_sdk::{Resource, metrics::SdkMeterProvider};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::env::Env;

pub(crate) struct Metrics {
    pub(crate) onchain_events_received: Counter<u64>,
    pub(crate) schwab_orders_executed: Counter<u64>,
    pub(crate) token_refreshes: Counter<u64>,
    pub(crate) queue_depth: Gauge<u64>,
    pub(crate) accumulated_positions: Gauge<f64>,
    pub(crate) trade_execution_duration_ms: Histogram<f64>,
    pub(crate) token_retry_counter: Arc<AtomicU64>,
    provider: SdkMeterProvider,
    flush_task: JoinHandle<()>,
}

pub(crate) fn setup(env: &Env) -> Option<Metrics> {
    let endpoint = env.otel_metrics_exporter_endpoint.as_ref()?;
    let api_key = env.otel_metrics_exporter_basic_auth_token.as_ref()?;
    let instance_id = env.otel_metrics_exporter_instance_id.as_ref()?;

    let deployment_environment = if env.dry_run { "dev" } else { "prod" };

    // Create provider for metric collection (export handled by custom HTTP task)

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

    global::set_meter_provider(provider.clone());
    let meter = global::meter("schwarbot");

    let onchain_events_received = meter.u64_counter("onchain_events_received").build();
    let schwab_orders_executed = meter.u64_counter("schwab_orders_executed").build();
    let token_refreshes = meter.u64_counter("token_refreshes").build();
    let queue_depth = meter.u64_gauge("queue_depth").build();
    let accumulated_positions = meter.f64_gauge("accumulated_positions").build();
    let trade_execution_duration_ms = meter.f64_histogram("trade_execution_duration_ms").build();

    info!("Successfully set up custom metrics with HTTP export to Grafana");

    // Record a startup metric for testing
    let startup_counter = meter.u64_counter("system_startup").build();
    startup_counter.add(1, &[KeyValue::new("status", "initialized")]);
    info!("Recorded startup metric for testing");

    // Create shared counter for token retries
    let token_retry_counter = Arc::new(AtomicU64::new(0));

    // Create custom background export task using our proven HTTP approach
    let endpoint_clone = endpoint.clone();
    let api_key_clone = api_key.clone();
    let instance_id_clone = instance_id.clone();
    let retry_counter_clone = token_retry_counter.clone();
    let flush_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
        interval.tick().await; // Skip first immediate tick

        // Track when we started to use as a consistent start_time for all metrics
        let service_start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        loop {
            interval.tick().await;

            // Create auth header using proven format
            let auth_string = format!("{}:{}", instance_id_clone, api_key_clone);
            let encoded_auth = general_purpose::STANDARD.encode(&auth_string);

            // Create current timestamp
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;

            // Get current retry count (cumulative)
            let retry_count = retry_counter_clone.load(Ordering::Relaxed);

            // Create metrics payload using proven JSON format (exact match to working test)
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
                        }]
                    }]
                }]
            });

            // Send HTTP request using proven approach
            let client = reqwest::Client::new();
            match client
                .post(format!("{}/v1/metrics", endpoint_clone))
                .header("Authorization", format!("Basic {}", encoded_auth))
                .header("Content-Type", "application/json")
                .json(&metrics_payload)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    let headers = response.headers().clone();

                    match response.text().await {
                        Ok(body) => {
                            if status.is_success() {
                                debug!(
                                    "Successfully exported metrics to Grafana - Status: {}, Body: {}",
                                    status, body
                                );
                            } else if status == 429 {
                                warn!(
                                    "Rate limited by Grafana (429) - Status: {}, Body: {}. Will retry next interval.",
                                    status, body
                                );
                            } else {
                                error!(
                                    "Failed to export metrics - Status: {}, Headers: {:?}, Body: {}",
                                    status, headers, body
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

    debug!("Metrics setup complete with custom HTTP export task");

    Some(Metrics {
        onchain_events_received,
        schwab_orders_executed,
        token_refreshes,
        queue_depth,
        accumulated_positions,
        trade_execution_duration_ms,
        token_retry_counter,
        provider,
        flush_task,
    })
}

impl Metrics {
    /// Increment the token retry counter
    pub(crate) fn increment_token_retry(&self) {
        self.token_retry_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for Metrics {
    fn drop(&mut self) {
        debug!("Shutting down metrics and background task");

        // Cancel the background flush task
        self.flush_task.abort();

        // Do a final flush
        if let Err(e) = self.provider.force_flush() {
            error!("Failed to do final metrics flush: {}", e);
        } else {
            debug!("Final metrics flush completed");
        }

        // Note: Don't call provider.shutdown() here as it can cause Tokio runtime panics
        // The provider will be properly cleaned up when dropped
    }
}
