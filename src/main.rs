use clap::Parser;
use rain_schwab::{Env, run};
use sqlx::SqlitePool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();

    let env = Env::try_parse()?;
    let pool = SqlitePool::connect(&env.database_url).await?;

    run(env, pool).await?;

    Ok(())
}
