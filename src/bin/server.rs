use clap::Parser;
use st0x_hedge::env::{Env, setup_tracing};
use st0x_hedge::launch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    setup_tracing(&env.log_level);

    launch(env).await?;
    Ok(())
}
