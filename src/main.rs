use clap::Parser;
use rain_schwab::Env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();

    let env = Env::try_parse()?;
    println!("{env:?}");

    Ok(())
}
