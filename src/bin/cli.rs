use st0x_arbot::cli;
use st0x_arbot::env::setup_tracing;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let (env, command) = cli::CliEnv::parse_and_convert()?;
    setup_tracing(&env.log_level);

    cli::run_command(env, command).await?;
    Ok(())
}
