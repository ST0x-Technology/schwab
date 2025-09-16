use clap::Parser;
use rain_schwab::env::{Env, setup_tracing};
use rain_schwab::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    setup_tracing(&env.log_level);

    run(env).await?;
    Ok(())
}
