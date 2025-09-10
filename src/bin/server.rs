use clap::Parser;
use rain_schwab::env::{Env, setup_tracing};
use rain_schwab::launch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let env = Env::try_parse()?;
    let shutdown_fn = setup_tracing(&env);

    let result = launch(env).await;

    if let Some(shutdown) = shutdown_fn {
        shutdown();
    }

    result
}
