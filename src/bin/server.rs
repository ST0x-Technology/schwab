use clap::Parser;
use rain_schwab::env::{Env, setup_tracing};
use rain_schwab::launch;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    setup_tracing(&env.log_level);

    launch(env).await?;
    Ok(())
}
