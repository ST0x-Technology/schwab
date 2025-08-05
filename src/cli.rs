use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use thiserror::Error;
use tracing::{error, info};

use crate::{
    Env,
    schwab::order::{Instruction, Order},
};

#[derive(Debug, Error)]
pub enum CliError {
    #[error(
        "Invalid ticker symbol: {symbol}. Ticker symbols must be uppercase letters only and 1-5 characters long"
    )]
    InvalidTicker { symbol: String },
    #[error("Invalid quantity: {value}. Quantity must be a positive number")]
    InvalidQuantity { value: String },
}

#[derive(Debug, Parser)]
#[command(name = "schwab")]
#[command(about = "A CLI tool for Charles Schwab stock trading")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Buy shares of a stock
    Buy {
        /// Stock ticker symbol (e.g., AAPL, TSLA)
        #[arg(short = 't', long = "ticker")]
        ticker: String,
        /// Number of shares to buy (fractional shares supported)
        #[arg(short = 'q', long = "quantity")]
        quantity: String,
    },
    /// Sell shares of a stock
    Sell {
        /// Stock ticker symbol (e.g., AAPL, TSLA)
        #[arg(short = 't', long = "ticker")]
        ticker: String,
        /// Number of shares to sell (fractional shares supported)
        #[arg(short = 'q', long = "quantity")]
        quantity: String,
    },
}

impl Cli {
    /// Parse and validate CLI arguments
    pub fn parse_and_validate() -> Result<ValidatedCliArgs, CliError> {
        let cli = Self::parse();

        match cli.command {
            Commands::Buy { ticker, quantity } => {
                let validated_ticker = validate_ticker(&ticker)?;
                let validated_quantity = validate_quantity(&quantity)?;
                Ok(ValidatedCliArgs::Buy {
                    ticker: validated_ticker,
                    quantity: validated_quantity,
                })
            }
            Commands::Sell { ticker, quantity } => {
                let validated_ticker = validate_ticker(&ticker)?;
                let validated_quantity = validate_quantity(&quantity)?;
                Ok(ValidatedCliArgs::Sell {
                    ticker: validated_ticker,
                    quantity: validated_quantity,
                })
            }
        }
    }
}

#[derive(Debug)]
pub enum ValidatedCliArgs {
    Buy { ticker: String, quantity: f64 },
    Sell { ticker: String, quantity: f64 },
}

fn validate_ticker(ticker: &str) -> Result<String, CliError> {
    let ticker = ticker.trim().to_uppercase();

    if ticker.is_empty() || ticker.len() > 5 {
        return Err(CliError::InvalidTicker { symbol: ticker });
    }

    if !ticker.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(CliError::InvalidTicker { symbol: ticker });
    }

    Ok(ticker)
}

fn validate_quantity(quantity_str: &str) -> Result<f64, CliError> {
    let quantity = quantity_str
        .trim()
        .parse::<f64>()
        .map_err(|_| CliError::InvalidQuantity {
            value: quantity_str.to_string(),
        })?;

    if quantity <= 0.0 || !quantity.is_finite() {
        return Err(CliError::InvalidQuantity {
            value: quantity_str.to_string(),
        });
    }

    Ok(quantity)
}

pub async fn run(env: Env) -> anyhow::Result<()> {
    let validated_args = Cli::parse_and_validate()?;
    let pool = env.get_sqlite_pool().await?;

    match validated_args {
        ValidatedCliArgs::Buy { ticker, quantity } => {
            info!("Processing buy order: ticker={ticker}, quantity={quantity}");
            execute_order(ticker, quantity, Instruction::Buy, &env, &pool).await?;
        }
        ValidatedCliArgs::Sell { ticker, quantity } => {
            info!("Processing sell order: ticker={ticker}, quantity={quantity}");
            execute_order(ticker, quantity, Instruction::Sell, &env, &pool).await?;
        }
    }

    info!("CLI operation completed successfully");
    Ok(())
}

async fn execute_order(
    ticker: String,
    quantity: f64,
    instruction: Instruction,
    env: &Env,
    pool: &SqlitePool,
) -> anyhow::Result<()> {
    let order = Order::new(ticker.clone(), instruction.clone(), quantity);

    info!("Created order: ticker={ticker}, instruction={instruction:?}, quantity={quantity}");

    match order.place(&env.schwab_auth, pool).await {
        Ok(()) => {
            info!(
                "Order placed successfully: ticker={ticker}, instruction={instruction:?}, quantity={quantity}"
            );
            println!("✅ Order placed successfully!");
            println!("   Ticker: {ticker}");
            println!("   Action: {instruction:?}");
            println!("   Quantity: {quantity}");
        }
        Err(e) => {
            error!(
                "Failed to place order: ticker={ticker}, instruction={instruction:?}, quantity={quantity}, error={e:?}"
            );
            eprintln!("❌ Failed to place order: {e}");
            return Err(e.into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use serde_json::json;

    #[tokio::test]
    async fn test_run_buy_order() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "*/*")
                .header("content-type", "application/json");
            then.status(201);
        });

        // Test the execute_order function directly since we can't easily mock CLI parsing in lib tests
        execute_order("AAPL".to_string(), 100.0, Instruction::Buy, &env, &pool)
            .await
            .unwrap();

        account_mock.assert();
        order_mock.assert();
    }

    #[tokio::test]
    async fn test_run_sell_order() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders")
                .header("authorization", "Bearer test_access_token")
                .header("accept", "*/*")
                .header("content-type", "application/json");
            then.status(201);
        });

        execute_order("TSLA".to_string(), 50.0, Instruction::Sell, &env, &pool)
            .await
            .unwrap();

        account_mock.assert();
        order_mock.assert();
    }

    #[tokio::test]
    async fn test_execute_order_failure() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/trader/v1/accounts/ABC123DEF456/orders");
            then.status(400)
                .json_body(json!({"error": "Invalid order"}));
        });

        let result =
            execute_order("INVALID".to_string(), 100.0, Instruction::Buy, &env, &pool).await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_err());
    }

    fn create_test_env_for_cli(mock_server: &MockServer) -> Env {
        use crate::{LogLevel, schwab::SchwabAuthEnv, trade::EvmEnv};
        use alloy::primitives::{address, fixed_bytes};

        Env {
            database_url: ":memory:".to_string(),
            log_level: LogLevel::Debug,
            schwab_auth: SchwabAuthEnv {
                app_key: "test_app_key".to_string(),
                app_secret: "test_app_secret".to_string(),
                redirect_uri: "https://127.0.0.1".to_string(),
                base_url: mock_server.base_url(),
                account_index: 0,
            },
            evm_env: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
                orderbook: address!("0x1234567890123456789012345678901234567890"),
                order_hash: fixed_bytes!(
                    "0x0000000000000000000000000000000000000000000000000000000000000000"
                ),
            },
        }
    }

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    async fn setup_test_tokens(pool: &SqlitePool) {
        let tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(pool).await.unwrap();
    }

    #[test]
    fn test_validate_ticker_valid() {
        assert_eq!(validate_ticker("AAPL").unwrap(), "AAPL");
        assert_eq!(validate_ticker("aapl").unwrap(), "AAPL");
        assert_eq!(validate_ticker("  TSLA  ").unwrap(), "TSLA");
        assert_eq!(validate_ticker("A").unwrap(), "A");
        assert_eq!(validate_ticker("GOOGL").unwrap(), "GOOGL");
    }

    #[test]
    fn test_validate_ticker_invalid() {
        assert!(matches!(
            validate_ticker(""),
            Err(CliError::InvalidTicker { .. })
        ));
        assert!(matches!(
            validate_ticker("TOOLONG"),
            Err(CliError::InvalidTicker { .. })
        ));
        assert!(matches!(
            validate_ticker("AAP1"),
            Err(CliError::InvalidTicker { .. })
        ));
        assert!(matches!(
            validate_ticker("AA-PL"),
            Err(CliError::InvalidTicker { .. })
        ));
        assert!(matches!(
            validate_ticker("AA PL"),
            Err(CliError::InvalidTicker { .. })
        ));
    }

    #[test]
    fn test_validate_quantity_valid() {
        assert!((validate_quantity("100").unwrap() - 100.0).abs() < f64::EPSILON);
        assert!((validate_quantity("100.5").unwrap() - 100.5).abs() < f64::EPSILON);
        assert!((validate_quantity("0.5").unwrap() - 0.5).abs() < f64::EPSILON);
        assert!((validate_quantity("  25.75  ").unwrap() - 25.75).abs() < f64::EPSILON);
        assert!((validate_quantity("1.0").unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_quantity_invalid() {
        assert!(matches!(
            validate_quantity("0"),
            Err(CliError::InvalidQuantity { .. })
        ));
        assert!(matches!(
            validate_quantity("-5"),
            Err(CliError::InvalidQuantity { .. })
        ));
        assert!(matches!(
            validate_quantity("abc"),
            Err(CliError::InvalidQuantity { .. })
        ));
        assert!(matches!(
            validate_quantity(""),
            Err(CliError::InvalidQuantity { .. })
        ));
        assert!(matches!(
            validate_quantity("inf"),
            Err(CliError::InvalidQuantity { .. })
        ));
        assert!(matches!(
            validate_quantity("nan"),
            Err(CliError::InvalidQuantity { .. })
        ));
    }

    #[test]
    fn test_validated_cli_args() {
        let args = ValidatedCliArgs::Buy {
            ticker: "AAPL".to_string(),
            quantity: 100.0,
        };

        match args {
            ValidatedCliArgs::Buy { ticker, quantity } => {
                assert_eq!(ticker, "AAPL");
                assert!((quantity - 100.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Buy variant"),
        }
    }

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
