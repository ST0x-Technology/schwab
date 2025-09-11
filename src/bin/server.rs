use clap::Parser;
use rain_schwab::env::{Env, setup_tracing};
use rain_schwab::launch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    setup_tracing(&env);

    // Test span to verify OpenTelemetry pipeline
    if env.hyperdx_api_key.is_some() {
        use opentelemetry::trace::{Span, Tracer};
        let tracer = opentelemetry::global::tracer("schwab-bot");
        let mut span = tracer.start("startup_test_span");
        span.set_attribute(opentelemetry::KeyValue::new("test", "true"));
        span.add_event(
            "Server starting",
            vec![opentelemetry::KeyValue::new(
                "dry_run",
                env.dry_run.to_string(),
            )],
        );
        span.end();
        println!("🧪 Created explicit OpenTelemetry test span");

        // Give batch exporter time to send the test span
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        println!("⏱️ Waited for span export");
    }

    launch(env).await
}
