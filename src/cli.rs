use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use std::io::Write;
use thiserror::Error;
use tracing::{error, info};

use crate::schwab::SchwabAuthEnv;
use crate::schwab::order::{Instruction, Order};
use crate::schwab::run_oauth_flow;
use crate::schwab::tokens::SchwabTokens;
use crate::trade::EvmEnv;
use crate::{Env, LogLevel};
use alloy::primitives::{address, fixed_bytes};

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

#[derive(Debug, Parser)]
#[command(name = "schwab-cli")]
#[command(about = "A CLI tool for Charles Schwab stock trading")]
#[command(version)]
pub struct CliEnv {
    #[clap(long = "db", env, default_value = "schwab.db")]
    pub database_url: String,
    #[clap(long, env, default_value = "info")]
    pub log_level: LogLevel,
    #[clap(flatten)]
    pub schwab_auth: SchwabAuthEnv,
    #[command(subcommand)]
    pub command: Commands,
}

impl CliEnv {
    /// Parse CLI arguments and convert to internal Env struct
    pub fn parse_and_convert() -> anyhow::Result<(Env, Commands)> {
        let cli_env = Self::parse();

        let env = Env {
            database_url: cli_env.database_url,
            log_level: cli_env.log_level,
            schwab_auth: cli_env.schwab_auth,
            evm_env: EvmEnv {
                ws_rpc_url: url::Url::parse("ws://localhost:8545")
                    .expect("Failed to parse dummy WS URL"),
                orderbook: address!("0x0000000000000000000000000000000000000000"),
                order_hash: fixed_bytes!(
                    "0x0000000000000000000000000000000000000000000000000000000000000000"
                ),
            },
        };

        Ok((env, cli_env.command))
    }
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
    run_with_writers(env, &mut std::io::stdout(), &mut std::io::stderr()).await
}

pub async fn run_command(env: Env, command: Commands) -> anyhow::Result<()> {
    run_command_with_writers(env, command, &mut std::io::stdout(), &mut std::io::stderr()).await
}

async fn run_with_writers<W1: Write, W2: Write>(
    env: Env,
    stdout: &mut W1,
    stderr: &mut W2,
) -> anyhow::Result<()> {
    let validated_args = Cli::parse_and_validate()?;
    let command = match validated_args {
        ValidatedCliArgs::Buy { ticker, quantity } => Commands::Buy {
            ticker,
            quantity: quantity.to_string(),
        },
        ValidatedCliArgs::Sell { ticker, quantity } => Commands::Sell {
            ticker,
            quantity: quantity.to_string(),
        },
    };

    run_command_with_writers(env, command, stdout, stderr).await
}

async fn run_command_with_writers<W1: Write, W2: Write>(
    env: Env,
    command: Commands,
    stdout: &mut W1,
    stderr: &mut W2,
) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    ensure_authentication(&pool, &env.schwab_auth, stderr).await?;

    match command {
        Commands::Buy { ticker, quantity } => {
            let validated_ticker = validate_ticker(&ticker)?;
            let validated_quantity = validate_quantity(&quantity)?;
            info!("Processing buy order: ticker={validated_ticker}, quantity={validated_quantity}");
            execute_order_with_writers(
                validated_ticker,
                validated_quantity,
                Instruction::Buy,
                &env,
                &pool,
                stdout,
                stderr,
            )
            .await?;
        }
        Commands::Sell { ticker, quantity } => {
            let validated_ticker = validate_ticker(&ticker)?;
            let validated_quantity = validate_quantity(&quantity)?;
            info!(
                "Processing sell order: ticker={validated_ticker}, quantity={validated_quantity}"
            );
            execute_order_with_writers(
                validated_ticker,
                validated_quantity,
                Instruction::Sell,
                &env,
                &pool,
                stdout,
                stderr,
            )
            .await?;
        }
    }

    info!("CLI operation completed successfully");
    Ok(())
}

async fn ensure_authentication<W: Write>(
    pool: &SqlitePool,
    schwab_auth: &crate::schwab::SchwabAuthEnv,
    stderr: &mut W,
) -> anyhow::Result<()> {
    info!("Refreshing authentication tokens if needed");

    match SchwabTokens::get_valid_access_token(pool, schwab_auth).await {
        Ok(_access_token) => {
            info!("Authentication tokens are valid, access token obtained");
            return Ok(());
        }
        Err(crate::schwab::SchwabError::RefreshTokenExpired) => {
            info!("Refresh token has expired, launching interactive OAuth flow");
            writeln!(
                stderr,
                "🔄 Your refresh token has expired. Starting authentication process..."
            )?;
            writeln!(
                stderr,
                "   You will be guided through the Charles Schwab OAuth process."
            )?;
        }
        Err(e) => {
            error!("Failed to obtain valid access token: {e:?}");
            writeln!(stderr, "❌ Authentication failed: {e}")?;
            return Err(e.into());
        }
    }

    match run_oauth_flow(pool, schwab_auth).await {
        Ok(()) => {
            info!("OAuth flow completed successfully");
            writeln!(
                stderr,
                "✅ Authentication successful! Continuing with your order..."
            )?;
            Ok(())
        }
        Err(oauth_error) => {
            error!("OAuth flow failed: {oauth_error:?}");
            writeln!(stderr, "❌ Authentication failed: {oauth_error}")?;
            writeln!(
                stderr,
                "   Please ensure you have a valid Charles Schwab account and try again."
            )?;
            Err(oauth_error.into())
        }
    }
}

async fn execute_order_with_writers<W1: Write, W2: Write>(
    ticker: String,
    quantity: f64,
    instruction: Instruction,
    env: &Env,
    pool: &SqlitePool,
    stdout: &mut W1,
    stderr: &mut W2,
) -> anyhow::Result<()> {
    let order = Order::new(ticker.clone(), instruction.clone(), quantity);

    info!("Created order: ticker={ticker}, instruction={instruction:?}, quantity={quantity}");

    match order.place(&env.schwab_auth, pool).await {
        Ok(()) => {
            info!(
                "Order placed successfully: ticker={ticker}, instruction={instruction:?}, quantity={quantity}"
            );
            writeln!(stdout, "✅ Order placed successfully!")?;
            writeln!(stdout, "   Ticker: {ticker}")?;
            writeln!(stdout, "   Action: {instruction:?}")?;
            writeln!(stdout, "   Quantity: {quantity}")?;
        }
        Err(e) => {
            error!(
                "Failed to place order: ticker={ticker}, instruction={instruction:?}, quantity={quantity}, error={e:?}"
            );
            writeln!(stderr, "❌ Failed to place order: {e}")?;
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

        execute_order_with_writers(
            "AAPL".to_string(),
            100.0,
            Instruction::Buy,
            &env,
            &pool,
            &mut std::io::sink(),
            &mut std::io::sink(),
        )
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

        execute_order_with_writers(
            "TSLA".to_string(),
            50.0,
            Instruction::Sell,
            &env,
            &pool,
            &mut std::io::sink(),
            &mut std::io::sink(),
        )
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

        let result = execute_order_with_writers(
            "INVALID".to_string(),
            100.0,
            Instruction::Buy,
            &env,
            &pool,
            &mut std::io::sink(),
            &mut std::io::sink(),
        )
        .await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_with_expired_refresh_token() {
        use chrono::{Duration, Utc};

        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        let expired_tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(35),
            refresh_token: "expired_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(8),
        };
        expired_tokens.store(&pool).await.unwrap();

        let result =
            crate::schwab::tokens::SchwabTokens::get_valid_access_token(&pool, &env.schwab_auth)
                .await;

        assert!(matches!(
            result.unwrap_err(),
            crate::schwab::SchwabError::RefreshTokenExpired
        ));
    }

    #[tokio::test]
    async fn test_run_with_successful_token_refresh() {
        use chrono::{Duration, Utc};

        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        let tokens_needing_refresh = crate::schwab::tokens::SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(35),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(1),
        };
        tokens_needing_refresh.store(&pool).await.unwrap();

        let refresh_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/oauth/token")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "access_token": "refreshed_access_token",
                    "refresh_token": "new_refresh_token"
                }));
        });

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
                .header("authorization", "Bearer refreshed_access_token");
            then.status(201);
        });

        let access_token =
            crate::schwab::tokens::SchwabTokens::get_valid_access_token(&pool, &env.schwab_auth)
                .await
                .unwrap();
        assert_eq!(access_token, "refreshed_access_token");

        execute_order_with_writers(
            "AAPL".to_string(),
            100.0,
            Instruction::Buy,
            &env,
            &pool,
            &mut std::io::sink(),
            &mut std::io::sink(),
        )
        .await
        .unwrap();

        refresh_mock.assert();
        account_mock.assert();
        order_mock.assert();

        let stored_tokens = crate::schwab::tokens::SchwabTokens::load(&pool)
            .await
            .unwrap();
        assert_eq!(stored_tokens.access_token, "refreshed_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_run_with_valid_tokens_no_refresh_needed() {
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
                .header("authorization", "Bearer test_access_token");
            then.status(201);
        });

        execute_order_with_writers(
            "TSLA".to_string(),
            50.0,
            Instruction::Sell,
            &env,
            &pool,
            &mut std::io::sink(),
            &mut std::io::sink(),
        )
        .await
        .unwrap();

        account_mock.assert();
        order_mock.assert();

        let stored_tokens = crate::schwab::tokens::SchwabTokens::load(&pool)
            .await
            .unwrap();
        assert_eq!(stored_tokens.access_token, "test_access_token");
    }

    #[tokio::test]
    async fn test_execute_order_success_stdout_output() {
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
            then.status(201);
        });

        let mut stdout_buffer = Vec::new();
        let mut stderr_buffer = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            123.45,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout_buffer,
            &mut stderr_buffer,
        )
        .await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_ok());

        let stdout_output = String::from_utf8(stdout_buffer).unwrap();
        let stderr_output = String::from_utf8(stderr_buffer).unwrap();

        assert!(stdout_output.contains("✅ Order placed successfully!"));
        assert!(stdout_output.contains("Ticker: AAPL"));
        assert!(stdout_output.contains("Action: Buy"));
        assert!(stdout_output.contains("Quantity: 123.45"));
        assert!(stderr_output.is_empty());
    }

    #[tokio::test]
    async fn test_execute_order_failure_stderr_output() {
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
                .json_body(json!({"error": "Invalid order parameters"}));
        });

        let mut stdout_buffer = Vec::new();
        let mut stderr_buffer = Vec::new();

        let result = execute_order_with_writers(
            "TSLA".to_string(),
            50.0,
            Instruction::Sell,
            &env,
            &pool,
            &mut stdout_buffer,
            &mut stderr_buffer,
        )
        .await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_err());

        let stdout_output = String::from_utf8(stdout_buffer).unwrap();
        let stderr_output = String::from_utf8(stderr_buffer).unwrap();

        assert!(stdout_output.is_empty());
        assert!(stderr_output.contains("❌ Failed to place order:"));
    }

    #[tokio::test]
    async fn test_authentication_with_oauth_flow_on_expired_refresh_token() {
        use chrono::{Duration, Utc};

        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        let expired_tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: Utc::now() - Duration::minutes(35),
            refresh_token: "expired_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now() - Duration::days(8),
        };
        expired_tokens.store(&pool).await.unwrap();

        let result =
            crate::schwab::tokens::SchwabTokens::get_valid_access_token(&pool, &env.schwab_auth)
                .await;

        assert!(matches!(
            result.unwrap_err(),
            crate::schwab::SchwabError::RefreshTokenExpired
        ));

        let mut stderr_buffer = Vec::new();
        writeln!(
            &mut stderr_buffer,
            "🔄 Your refresh token has expired. Starting authentication process..."
        )
        .unwrap();
        writeln!(
            &mut stderr_buffer,
            "   You will be guided through the Charles Schwab OAuth process."
        )
        .unwrap();

        let stderr_output = String::from_utf8(stderr_buffer).unwrap();
        assert!(
            stderr_output
                .contains("🔄 Your refresh token has expired. Starting authentication process...")
        );
        assert!(
            stderr_output.contains("You will be guided through the Charles Schwab OAuth process.")
        );
    }

    #[test]
    fn test_cli_error_display_messages() {
        let ticker_error = CliError::InvalidTicker {
            symbol: "TOOLONG".to_string(),
        };
        let error_msg = ticker_error.to_string();
        assert!(error_msg.contains("Invalid ticker symbol: TOOLONG"));
        assert!(error_msg.contains("uppercase letters only"));
        assert!(error_msg.contains("1-5 characters long"));

        let quantity_error = CliError::InvalidQuantity {
            value: "-5".to_string(),
        };
        let error_msg = quantity_error.to_string();
        assert!(error_msg.contains("Invalid quantity: -5"));
        assert!(error_msg.contains("positive number"));
    }

    #[test]
    fn test_validated_cli_args_display() {
        let buy_args = ValidatedCliArgs::Buy {
            ticker: "AAPL".to_string(),
            quantity: 100.0,
        };

        match buy_args {
            ValidatedCliArgs::Buy { ticker, quantity } => {
                assert_eq!(ticker, "AAPL");
                assert_eq!(quantity, 100.0);
            }
            _ => panic!("Expected Buy variant"),
        }

        let sell_args = ValidatedCliArgs::Sell {
            ticker: "TSLA".to_string(),
            quantity: 50.5,
        };

        match sell_args {
            ValidatedCliArgs::Sell { ticker, quantity } => {
                assert_eq!(ticker, "TSLA");
                assert_eq!(quantity, 50.5);
            }
            _ => panic!("Expected Sell variant"),
        }
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

    #[test]
    fn test_parse_and_validate_buy_command() {
        let validated_ticker = validate_ticker("aapl").unwrap();
        let validated_quantity = validate_quantity("100.5").unwrap();

        assert_eq!(validated_ticker, "AAPL");
        assert!((validated_quantity - 100.5).abs() < f64::EPSILON);

        let validated_args = ValidatedCliArgs::Buy {
            ticker: validated_ticker,
            quantity: validated_quantity,
        };

        match validated_args {
            ValidatedCliArgs::Buy { ticker, quantity } => {
                assert_eq!(ticker, "AAPL");
                assert!((quantity - 100.5).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Buy variant"),
        }
    }

    #[test]
    fn test_parse_and_validate_sell_command() {
        let validated_ticker = validate_ticker("TSLA").unwrap();
        let validated_quantity = validate_quantity("50").unwrap();

        assert_eq!(validated_ticker, "TSLA");
        assert!((validated_quantity - 50.0).abs() < f64::EPSILON);

        let validated_args = ValidatedCliArgs::Sell {
            ticker: validated_ticker,
            quantity: validated_quantity,
        };

        match validated_args {
            ValidatedCliArgs::Sell { ticker, quantity } => {
                assert_eq!(ticker, "TSLA");
                assert!((quantity - 50.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Sell variant"),
        }
    }

    #[test]
    fn test_validate_ticker_boundary_conditions() {
        assert_eq!(validate_ticker("GOOGL").unwrap(), "GOOGL");

        assert!(matches!(
            validate_ticker("GOOGLE"),
            Err(CliError::InvalidTicker { .. })
        ));

        assert_eq!(validate_ticker("   aapl   ").unwrap(), "AAPL");

        assert_eq!(validate_ticker("a").unwrap(), "A");
    }

    #[test]
    fn test_validate_quantity_edge_cases() {
        assert!((validate_quantity("0.001").unwrap() - 0.001).abs() < f64::EPSILON);

        assert!((validate_quantity("999999.99").unwrap() - 999999.99).abs() < f64::EPSILON);

        assert!((validate_quantity("1e2").unwrap() - 100.0).abs() < f64::EPSILON);

        assert!(matches!(
            validate_quantity("1e999"),
            Err(CliError::InvalidQuantity { .. })
        ));

        assert!((validate_quantity("   123.456   ").unwrap() - 123.456).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cli_command_structure_validation() {
        use clap::CommandFactory;

        let cmd = Cli::command();

        let result = cmd
            .clone()
            .try_get_matches_from(vec!["schwab", "buy", "-t", "AAPL"]);
        assert!(result.is_err());

        let result = cmd
            .clone()
            .try_get_matches_from(vec!["schwab", "sell", "-q", "100"]);
        assert!(result.is_err());

        let result = cmd.clone().try_get_matches_from(vec!["schwab", "buy"]);
        assert!(result.is_err());

        let result = cmd
            .clone()
            .try_get_matches_from(vec!["schwab", "buy", "-t", "AAPL", "-q", "100"]);
        assert!(result.is_ok());

        let result = cmd
            .clone()
            .try_get_matches_from(vec!["schwab", "sell", "-t", "TSLA", "-q", "50"]);
        assert!(result.is_ok());
    }
}
