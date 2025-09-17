use opentelemetry as otlp;
use opentelemetry::metrics::Counter;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::{metrics::reader::DefaultTemporalitySelector, Resource};
use std::{collections::HashMap, time::Duration};
use tracing::*;

use crate::env::ENV;

pub type MeterProvider = std::sync::Arc<dyn otlp::metrics::MeterProvider + Send + Sync>;

#[derive(Clone)]
pub struct Metrics {
    pub market_event_counter: Counter<u64>,
    pub provider: MeterProvider,
}

pub fn setup() -> anyhow::Result<Metrics> {
    debug!("Setting up metrics...");

    let provider = opentelemetry_otlp::new_pipeline()
        .metrics(opentelemetry_sdk::runtime::Tokio)
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(&ENV.otel_exporter_otlp_endpoint)
                .with_protocol(Protocol::HttpBinary)
                .with_headers(HashMap::from([(
                    "Authorization".to_string(),
                    format!("Basic {}", ENV.otel_exporter_otlp_basic_auth_token),
                )])),
        )
        .with_resource(Resource::new(vec![
            otlp::KeyValue::new("service.name", "degen"),
            otlp::KeyValue::new("deployment.environment", "dev"),
        ]))
        .with_period(Duration::from_secs(3))
        .with_timeout(Duration::from_secs(10))
        .with_temporality_selector(DefaultTemporalitySelector::new())
        .build()?;

    debug!("{provider:#?}");

    otlp::global::set_meter_provider(provider);
    let provider = otlp::global::meter_provider();

    let meter = provider.meter("degen");
    let market_event_counter = meter.u64_counter("market_event_counter").init();

    info!("Successfully set up OTLP metrics");

    Ok(Metrics {
        market_event_counter,
        provider,
    })
}