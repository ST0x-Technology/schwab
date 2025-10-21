use clap::Parser;
use st0x_hedge::env::setup_tracing;
use st0x_hedge::reporter::{self, ReporterEnv};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = ReporterEnv::parse();
    setup_tracing(env.log_level());

    reporter::run(env).await
}
