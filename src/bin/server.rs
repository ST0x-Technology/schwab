use clap::Parser;
use st0x_hedge::env::{Env, setup_tracing};
use st0x_hedge::launch;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let parsed_env = Env::parse();
    let config = parsed_env.into_config();

    let telemetry_guard = if let Some(api_key) = &config.hyperdx_api_key {
        match st0x_hedge::setup_telemetry(api_key.clone(), (&config.log_level).into()) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("Failed to setup telemetry: {e}");
                setup_tracing(&config.log_level);
                None
            }
        }
    } else {
        setup_tracing(&config.log_level);
        None
    };

    let result = launch(config).await;

    if telemetry_guard.is_some() {
        info!("Waiting 10 seconds for telemetry export before shutdown...");
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }

    result?;
    Ok(())
}
