use clap::Parser;
use rain_schwab::{Env, run, setup_tracing};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_tracing();

    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    run(env).await?;
    Ok(())
}
