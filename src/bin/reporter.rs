use clap::Parser;
use rain_schwab::env::setup_tracing;
use rain_schwab::reporter::{self, ReporterEnv};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = ReporterEnv::parse();
    setup_tracing(env.log_level());

    reporter::run(env).await
}
