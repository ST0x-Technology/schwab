use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::ExporterBuildError;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Layer, SubscriberExt};

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("Failed to build OTLP exporter")]
    OtlpExporter(#[from] ExporterBuildError),

    #[error("Failed to build HTTP client")]
    HttpClient(String),

    #[error("Failed to spawn HTTP client thread")]
    ThreadSpawn,

    #[error("Failed to set global subscriber")]
    Subscriber(#[from] tracing::subscriber::SetGlobalDefaultError),
}

pub struct TelemetryGuard {
    tracer_provider: SdkTracerProvider,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        let _ = self.tracer_provider.force_flush();
    }
}

/// Instrumentation library name used to identify the source of traces in the
/// OpenTelemetry system. This appears in telemetry backends as the library
/// that generated the spans.
///
/// This is distinct from the service name:
/// - Service name (e.g., "st0x-hedge"): Identifies which service the traces
///   come from in a distributed system. Shows as `service.name` resource
///   attribute.
/// - Tracer name (this constant): Identifies which instrumentation library
///   within the service created the spans. Used to distinguish between
///   application code ("st0x_tracer") and auto-instrumented libraries
///   (e.g., "reqwest", "sqlx").
///
/// Since we use a single tracer for all application code without library
/// auto-instrumentation, this distinction is somewhat artificial but
/// maintained for semantic clarity.
const TRACER_NAME: &str = "st0x-tracer";

pub fn setup_telemetry(
    api_key: String,
    log_level: tracing::Level,
) -> Result<TelemetryGuard, TelemetryError> {
    let headers = HashMap::from([("authorization".to_string(), api_key)]);

    let http_client = std::thread::spawn(|| {
        reqwest::blocking::Client::builder()
            .gzip(true)
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))
    })
    .join()
    .map_err(|_| TelemetryError::ThreadSpawn)?
    .map_err(TelemetryError::HttpClient)?;

    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(http_client)
        .with_endpoint("https://in-otel.hyperdx.io/v1/traces")
        .with_headers(headers)
        .with_protocol(opentelemetry_otlp::Protocol::Grpc)
        .build()?;

    let batch_exporter = BatchSpanProcessor::builder(otlp_exporter)
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_max_export_batch_size(512)
                .with_max_queue_size(2048)
                .with_scheduled_delay(Duration::from_secs(3))
                .build(),
        )
        .build();

    let tracer_provider = SdkTracerProvider::builder()
        .with_span_processor(batch_exporter)
        .with_resource(
            Resource::builder()
                .with_service_name("st0x-hedge")
                .with_attributes(vec![KeyValue::new("deployment.environment", "production")])
                .build(),
        )
        .build();

    let tracer = tracer_provider.tracer(TRACER_NAME);

    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let default_filter = format!("st0x_hedge={log_level},st0x_broker={log_level}");

    let fmt_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.clone().into());

    let telemetry_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| default_filter.into());

    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(fmt_filter);
    let telemetry_layer = telemetry_layer.with_filter(telemetry_filter);

    let subscriber = Registry::default().with(fmt_layer).with(telemetry_layer);

    tracing::subscriber::set_global_default(subscriber)?;

    Ok(TelemetryGuard { tracer_provider })
}
