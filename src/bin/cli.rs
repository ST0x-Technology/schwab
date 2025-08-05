use clap::Parser;
use rain_schwab::{Env, cli, setup_tracing};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    setup_tracing(env.log_level.clone());

    cli::run(env).await?;
    Ok(())
}
