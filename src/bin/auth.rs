use clap::Parser;
use rain_schwab::{Env, setup_tracing};
use rain_schwab::schwab::run_oauth_flow;
use tracing::debug;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_tracing();

    debug!("Reading env...");
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;

    debug!("Connecting to SQLite...");
    let pool = env.get_sqlite_pool().await?;

    run_oauth_flow(&pool, &env.schwab_auth).await?;

    Ok(())
}
