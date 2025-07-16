use clap::Parser;
use rain_schwab::Env;
use rain_schwab::schwab::run_oauth_flow;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();

    let env = Env::try_parse()?;
    let pool = env.get_sqlite_pool().await?;

    run_oauth_flow(&pool, &env.schwab_auth).await?;

    Ok(())
}
