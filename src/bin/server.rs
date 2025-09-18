use clap::Parser;
use rain_schwab::env::{Env, setup_tracing};
use rain_schwab::launch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    let tracer_provider = setup_tracing(&env);

    // Test both explicit OpenTelemetry span AND tracing span
    if env.hyperdx_api_key.is_some() {
        use opentelemetry::trace::{Span, Tracer};

        eprintln!("🔍 Creating test spans...");

        // 1. Explicit OpenTelemetry span
        let tracer = opentelemetry::global::tracer("schwab-bot");
        let mut span = tracer.start("explicit_otel_span_new");
        span.set_attribute(opentelemetry::KeyValue::new("test", "explicit"));
        span.add_event("Explicit span event", vec![]);
        span.end();
        eprintln!("✅ Created explicit OpenTelemetry span");

        // 2. Tracing span
        let tracing_span = tracing::info_span!("tracing_test_span_new", test = "tracing");
        let _enter = tracing_span.enter();
        tracing::info!("This is a tracing event inside a tracing span");
        drop(_enter);
        eprintln!("✅ Created tracing span");

        // 3. Force flush and check for errors
        if let Some(ref provider) = tracer_provider {
            eprintln!("🔍 Forcing flush...");
            match provider.force_flush() {
                Ok(_) => eprintln!("✅ Flush succeeded"),
                Err(e) => eprintln!("❌ Flush failed: {e:?}"),
            }

            eprintln!("🔍 Waiting 10 seconds for export...");
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            eprintln!("✅ Wait complete");
        }
    }

    launch(env).await
}
