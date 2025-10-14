use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor};
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{Instrument, error, info, span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::SubscriberExt;

const IS_BATCH_EXPORTER: bool = true; // false for simple exporter, true for batch exporter

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("HYPERDX_API_KEY").unwrap();

    let mut headers = std::collections::HashMap::new();
    // for hyperdx, api key must be used without Bearer token
    headers.insert("authorization".to_string(), api_key);

    let otlp_http_exporter_builder = if IS_BATCH_EXPORTER {
        // for batch exporter we need blocking reqwest client initiated on a separate thread
        // because batch exporter uses a thread pool to export spans in the background that
        // would conflict with tokio main runtime if initiated normally because BatchSpanProcessor
        // will try to close this main tokio runtime once it starts operating
        // read the BatchSpanProcessor docs for more info
        let http_client = std::thread::spawn(move || {
            reqwest::blocking::Client::builder()
                .gzip(true) // enable gzip compression for less data usage
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new())
        })
        .join()
        .unwrap();
        opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(http_client)
    } else {
        // for simple exporter we can use normal async reqwest client
        // there seems to be some difference in simple vs batch exporter that the simple exporter
        // also automatically captures reqwest internal calls as DEBUG events when a span ends and
        // gets exported, not sure why though, probably because normal reqwest client has internal
        // tracing enabled by default
        let http_client = reqwest::Client::builder()
            .gzip(true) // enable gzip compression for less data usage
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_http_client(http_client)
    };

    // build http exporter
    let otlp_exporter = otlp_http_exporter_builder
        .with_endpoint("https://in-otel.hyperdx.io/v1/traces")
        .with_headers(headers)
        .with_protocol(opentelemetry_otlp::Protocol::Grpc) // fastest
        .build()
        .unwrap();

    let tracer_provider_builder = if IS_BATCH_EXPORTER {
        // create batch span processor with custom config
        // also can build simple span processor or even custom
        // processors with implementation of SpanProcessor trait
        let batch_exporter = BatchSpanProcessor::builder(otlp_exporter)
            .with_batch_config(
                BatchConfigBuilder::default()
                    .with_max_export_batch_size(512)
                    .with_max_queue_size(2048)
                    .with_scheduled_delay(Duration::from_secs(3))
                    // .with_max_concurrent_exports() // experimental feature flag for the otel crate
                    .build(),
            )
            .build();

        // build batch exporter provider builder with batch exporter
        opentelemetry_sdk::trace::SdkTracerProvider::builder().with_span_processor(batch_exporter)
    } else {
        // build simple exporter provider builder with otlp exporter
        opentelemetry_sdk::trace::SdkTracerProvider::builder().with_simple_exporter(otlp_exporter)
    };

    // build tracer provider
    let tracer_provider = tracer_provider_builder
        .with_resource(
            Resource::builder()
                .with_service_name("my-service-name")
                .with_attributes(vec![
                    KeyValue::new("deployment.environment", "production"), // these attributes will be in every span
                ])
                .build(),
        )
        .build();

    // make a tracer
    let tracer = tracer_provider.tracer("my_tracer");

    // Create a tracing layer with the configured tracer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    // Use the tracing subscriber `Registry`, or any other subscriber that impls `LookupSpan`
    let subscriber = Registry::default().with(telemetry);

    // set as global default to make sure all spans are captured
    tracing::subscriber::set_global_default(subscriber).unwrap();

    // lets make a top level span to be the parent of all other spans
    // for making parent-child relationships when spans are not already
    // nested, `context` must be used and passed to a child span as its context
    {
        let root_span = span!(tracing::Level::INFO, "app_start", work_units = 2);

        // we must enter the root_span so that children get correctly captured for test_fn1()
        // call and the inline span, otherwise it will become its own separate span
        let _enter = root_span.enter();

        // add a custom event
        root_span.add_event("started", vec![KeyValue::new("hello", "world")]); // this always works with or without enter()

        // inline child span
        {
            let root_child_span = span!(tracing::Level::INFO, "app_start_child", work_units = 2);
            let _enter = root_child_span.enter(); // enter the span to enable the event macros otherwise the span.add_event method need to be used to add events

            // add 2 events using macro and method call
            error!("This event will be logged as error event in the root_child_span."); // this will work because of enter()
            root_child_span.add_event("eventName", vec![KeyValue::new("attr1", "123")]); // this will also work with or without enter()
        };

        // simulate some async work, also child of root_span, but not root_child_span
        test_fn1().await;
    }

    // lets make a separate span with this function call
    // this will NOT be a child of root_span because it is outside the scope
    let _ = test_fn3().await;

    // force a flush to ensure all spans are exported before exit
    let _ = tracer_provider.force_flush();

    // wait a bit to ensure all spans are exported before exit
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
}

// test fn with instrument macro and nested call, child of root_span
// nested spans automatically become children of parent span
#[tracing::instrument(skip_all, fields(component = "websocket"), level = tracing::Level::DEBUG)]
async fn test_fn1() {
    info!("this is a debug from test_fn1"); // this will work because of the instrument macro already entered the span
    tokio::time::sleep(std::time::Duration::from_secs(4)).await; // do some async task

    // will be child of test_fn1 span and also root_span
    test_fn2(2);
}

// test fn with manual span creation
fn test_fn2(a: u8) {
    let spn = span!(tracing::Level::WARN, "test_fn2");
    spn.set_attribute("attr1", a.to_string()); // add an attribute
}

// test fn with using instrument trait
fn test_fn3() -> JoinHandle<()> {
    let span = span!(tracing::Level::INFO, "test_fn3");
    let _enter = span.enter();
    info!("Starting queue processor service");
    tokio::spawn(async move {
        // although its nested but will not become child of test_fn3 span, because of the thread move
        // that the outter span _enter guard gets dropped before this span is started
        let span = tracing::info_span!("queue_processor_task", component = "conductor");
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await; // do some async task
            error!("Queue processor service failed"); // works because of instrument(span) call below
        }
        .instrument(span)
        .await;
    })
}
