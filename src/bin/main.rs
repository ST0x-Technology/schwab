use clap::Parser;
use rain_schwab::{Env, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    run(env).await?;
    Ok(())
}
