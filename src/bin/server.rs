use clap::Parser;
use st0x_hedge::env::{Env, setup_tracing};
use st0x_hedge::launch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let parsed_env = Env::parse();
    let config = parsed_env.into_config();
    setup_tracing(&config.log_level);

    launch(config).await?;
    Ok(())
}
