use clap::Parser;
use rain_schwab::Env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let env = Env::try_parse()?;
    println!("{env:?}");
    Ok(())
}
