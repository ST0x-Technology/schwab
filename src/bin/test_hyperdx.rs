use opentelemetry::KeyValue;
use tokio::task::JoinHandle;
use tracing::{Instrument, error, info, span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let api_key = std::env::var("HYPERDX_API_KEY").unwrap();

    let _telemetry_guard = st0x_hedge::setup_telemetry(api_key).unwrap();

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
