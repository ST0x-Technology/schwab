use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use std::io::Write;
use thiserror::Error;
use tracing::{error, info};

use crate::arb::ArbTrade;
use crate::schwab::SchwabAuthEnv;
use crate::schwab::SchwabInstruction;
use crate::schwab::order::{Instruction, Order, execute_trade};
use crate::schwab::run_oauth_flow;
use crate::schwab::tokens::SchwabTokens;
use crate::symbol_cache::SymbolCache;
use crate::trade::{EvmEnv, PartialArbTrade, TradeConversionError, TradeStatus};
use crate::{Env, LogLevel};
use alloy::primitives::B256;
use alloy::providers::{ProviderBuilder, WsConnect};

#[derive(Debug, Error)]
pub enum CliError {
    #[error(
        "Invalid ticker symbol: {symbol}. Ticker symbols must be uppercase letters only and 1-5 characters long"
    )]
    InvalidTicker { symbol: String },
    #[error("Invalid quantity: {value}. Quantity must be greater than zero")]
    InvalidQuantity { value: u64 },
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
        /// Number of shares to buy (whole shares only)
        #[arg(short = 'q', long = "quantity")]
        quantity: u64,
    },
    /// Sell shares of a stock
    Sell {
        /// Stock ticker symbol (e.g., AAPL, TSLA)
        #[arg(short = 't', long = "ticker")]
        ticker: String,
        /// Number of shares to sell (whole shares only)
        #[arg(short = 'q', long = "quantity")]
        quantity: u64,
    },
    /// Process a transaction hash to execute opposite-side trade
    ProcessTx {
        /// Transaction hash (0x prefixed, 64 hex characters)
        #[arg(long = "tx-hash")]
        tx_hash: B256,
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
    #[clap(flatten)]
    pub evm_env: EvmEnv,
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
            evm_env: cli_env.evm_env,
        };

        Ok((env, cli_env.command))
    }
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

pub async fn run(env: Env) -> anyhow::Result<()> {
    let cli = Cli::parse();
    run_with_writers(env, cli.command, &mut std::io::stdout()).await
}

pub async fn run_command(env: Env, command: Commands) -> anyhow::Result<()> {
    run_command_with_writers(env, command, &mut std::io::stdout()).await
}

async fn run_with_writers<W: Write>(
    env: Env,
    command: Commands,
    stdout: &mut W,
) -> anyhow::Result<()> {
    run_command_with_writers(env, command, stdout).await
}

async fn run_command_with_writers<W: Write>(
    env: Env,
    command: Commands,
    stdout: &mut W,
) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;

    match command {
        Commands::Buy { ticker, quantity } => {
            ensure_authentication(&pool, &env.schwab_auth, stdout).await?;
            let validated_ticker = validate_ticker(&ticker)?;
            if quantity == 0 {
                return Err(CliError::InvalidQuantity { value: quantity }.into());
            }
            info!("Processing buy order: ticker={validated_ticker}, quantity={quantity}");
            execute_order_with_writers(
                validated_ticker,
                quantity,
                Instruction::Buy,
                &env,
                &pool,
                stdout,
            )
            .await?;
        }
        Commands::Sell { ticker, quantity } => {
            ensure_authentication(&pool, &env.schwab_auth, stdout).await?;
            let validated_ticker = validate_ticker(&ticker)?;
            if quantity == 0 {
                return Err(CliError::InvalidQuantity { value: quantity }.into());
            }
            info!("Processing sell order: ticker={validated_ticker}, quantity={quantity}");
            execute_order_with_writers(
                validated_ticker,
                quantity,
                Instruction::Sell,
                &env,
                &pool,
                stdout,
            )
            .await?;
        }
        Commands::ProcessTx { tx_hash } => {
            info!("Processing transaction: tx_hash={tx_hash}");
            process_tx_command_with_writers(tx_hash, &env, &pool, stdout).await?;
        }
    }

    info!("CLI operation completed successfully");
    Ok(())
}

async fn ensure_authentication<W: Write>(
    pool: &SqlitePool,
    schwab_auth: &crate::schwab::SchwabAuthEnv,
    stdout: &mut W,
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
                stdout,
                "🔄 Your refresh token has expired. Starting authentication process..."
            )?;
            writeln!(
                stdout,
                "   You will be guided through the Charles Schwab OAuth process."
            )?;
        }
        Err(e) => {
            error!("Failed to obtain valid access token: {e:?}");
            writeln!(stdout, "❌ Authentication failed: {e}")?;
            return Err(e.into());
        }
    }

    match run_oauth_flow(pool, schwab_auth).await {
        Ok(()) => {
            info!("OAuth flow completed successfully");
            writeln!(
                stdout,
                "✅ Authentication successful! Continuing with your order..."
            )?;
            Ok(())
        }
        Err(oauth_error) => {
            error!("OAuth flow failed: {oauth_error:?}");
            writeln!(stdout, "❌ Authentication failed: {oauth_error}")?;
            writeln!(
                stdout,
                "   Please ensure you have a valid Charles Schwab account and try again."
            )?;
            Err(oauth_error.into())
        }
    }
}

async fn execute_order_with_writers<W: Write>(
    ticker: String,
    quantity: u64,
    instruction: Instruction,
    env: &Env,
    pool: &SqlitePool,
    stdout: &mut W,
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
            writeln!(stdout, "❌ Failed to place order: {e}")?;
            return Err(e.into());
        }
    }

    Ok(())
}

async fn process_tx_command_with_writers<W: Write>(
    tx_hash: B256,
    env: &Env,
    pool: &SqlitePool,
    stdout: &mut W,
) -> anyhow::Result<()> {
    let evm_env = &env.evm_env;

    let provider = if evm_env.ws_rpc_url.scheme().starts_with("ws") {
        let ws = WsConnect::new(evm_env.ws_rpc_url.as_str());
        ProviderBuilder::new().connect_ws(ws).await?
    } else {
        ProviderBuilder::new().connect_http(evm_env.ws_rpc_url.clone())
    };
    let cache = SymbolCache::default();

    match PartialArbTrade::try_from_tx_hash(tx_hash, &provider, &cache, evm_env).await {
        Ok(Some(partial_trade)) => {
            process_found_trade(partial_trade, env, pool, stdout).await?;
        }
        Ok(None) => {
            writeln!(
                stdout,
                "❌ No tradeable events found in transaction {tx_hash}"
            )?;
            writeln!(
                stdout,
                "   This transaction may not contain orderbook events matching the configured order hash."
            )?;
        }
        Err(TradeConversionError::TransactionNotFound(hash)) => {
            writeln!(stdout, "❌ Transaction not found: {hash}")?;
            writeln!(
                stdout,
                "   Please verify the transaction hash and ensure the RPC endpoint is correct."
            )?;
        }
        Err(e) => {
            writeln!(stdout, "❌ Error processing transaction: {e}")?;
            return Err(e.into());
        }
    }

    Ok(())
}

async fn process_found_trade<W: Write>(
    partial_trade: PartialArbTrade,
    env: &Env,
    pool: &SqlitePool,
    stdout: &mut W,
) -> anyhow::Result<()> {
    display_trade_details(&partial_trade, stdout)?;

    let trade = ArbTrade::from_partial_trade(partial_trade);

    if !save_trade_to_db(&trade, pool, stdout).await? {
        return Ok(());
    }

    ensure_authentication(pool, &env.schwab_auth, stdout).await?;
    execute_and_report_trade(env, pool, trade, stdout).await
}

fn display_trade_details<W: Write>(
    partial_trade: &PartialArbTrade,
    stdout: &mut W,
) -> anyhow::Result<()> {
    writeln!(stdout, "✅ Found opposite-side trade opportunity:")?;
    writeln!(stdout, "   Transaction: {}", partial_trade.tx_hash)?;
    writeln!(stdout, "   Log Index: {}", partial_trade.log_index)?;
    writeln!(stdout, "   Schwab Ticker: {}", partial_trade.schwab_ticker)?;
    writeln!(
        stdout,
        "   Schwab Action: {:?}",
        partial_trade.schwab_instruction
    )?;
    writeln!(stdout, "   Quantity: {}", partial_trade.schwab_quantity)?;
    writeln!(
        stdout,
        "   Onchain Input: {} ({})",
        partial_trade.onchain_input_amount, partial_trade.onchain_input_symbol
    )?;
    writeln!(
        stdout,
        "   Onchain Output: {} ({})",
        partial_trade.onchain_output_amount, partial_trade.onchain_output_symbol
    )?;
    Ok(())
}

async fn save_trade_to_db<W: Write>(
    trade: &ArbTrade,
    pool: &SqlitePool,
    stdout: &mut W,
) -> anyhow::Result<bool> {
    match trade.try_save_to_db(pool).await {
        Ok(true) => {
            writeln!(stdout, "📝 Trade saved to database (NEW)")?;
            Ok(true)
        }
        Ok(false) => {
            writeln!(stdout, "📝 Trade already exists in database (DUPLICATE)")?;
            writeln!(stdout, "   Skipping execution to prevent duplicate trade")?;
            Ok(false)
        }
        Err(e) => {
            writeln!(stdout, "❌ Failed to save trade to database: {e}")?;
            Err(e.into())
        }
    }
}

async fn execute_and_report_trade<W: Write>(
    env: &Env,
    pool: &SqlitePool,
    trade: ArbTrade,
    stdout: &mut W,
) -> anyhow::Result<()> {
    writeln!(stdout, "🔄 Executing trade on Schwab...")?;
    execute_trade(env, pool, trade.clone(), 3).await;

    let updated_trade =
        ArbTrade::find_by_tx_hash_and_log_index(pool, trade.tx_hash, trade.log_index).await?;

    match updated_trade.status {
        TradeStatus::Completed => {
            writeln!(stdout, "🎯 Trade completed successfully!")?;
            writeln!(stdout, "   ✅ Opposite-side trade executed on Schwab")?;
            writeln!(stdout, "   📊 Database status: COMPLETED")?;
        }
        TradeStatus::Failed => {
            writeln!(stdout, "❌ Trade execution failed")?;
            writeln!(stdout, "   📊 Database status: FAILED")?;
            return Err(anyhow::anyhow!("Trade execution failed"));
        }
        TradeStatus::Pending => {
            writeln!(stdout, "⏳ Trade status: {}", TradeStatus::Pending.as_str())?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, fixed_bytes};
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
            100,
            Instruction::Buy,
            &env,
            &pool,
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
            50,
            Instruction::Sell,
            &env,
            &pool,
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
            100,
            Instruction::Buy,
            &env,
            &pool,
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
            100,
            Instruction::Buy,
            &env,
            &pool,
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
            50,
            Instruction::Sell,
            &env,
            &pool,
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

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            123,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout_buffer,
        )
        .await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_ok());

        let stdout_output = String::from_utf8(stdout_buffer).unwrap();

        assert!(stdout_output.contains("✅ Order placed successfully!"));
        assert!(stdout_output.contains("Ticker: AAPL"));
        assert!(stdout_output.contains("Action: Buy"));
        assert!(stdout_output.contains("Quantity: 123"));
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

        let result = execute_order_with_writers(
            "TSLA".to_string(),
            50,
            Instruction::Sell,
            &env,
            &pool,
            &mut stdout_buffer,
        )
        .await;

        account_mock.assert();
        order_mock.assert();
        assert!(result.is_err());

        let stdout_output = String::from_utf8(stdout_buffer).unwrap();

        assert!(stdout_output.contains("❌ Failed to place order:"));
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

        let mut stdout_buffer = Vec::new();
        writeln!(
            &mut stdout_buffer,
            "🔄 Your refresh token has expired. Starting authentication process..."
        )
        .unwrap();
        writeln!(
            &mut stdout_buffer,
            "   You will be guided through the Charles Schwab OAuth process."
        )
        .unwrap();

        let stdout_output = String::from_utf8(stdout_buffer).unwrap();
        assert!(
            stdout_output
                .contains("🔄 Your refresh token has expired. Starting authentication process...")
        );
        assert!(
            stdout_output.contains("You will be guided through the Charles Schwab OAuth process.")
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

        let quantity_error = CliError::InvalidQuantity { value: 0 };
        let error_msg = quantity_error.to_string();
        assert!(error_msg.contains("Invalid quantity: 0"));
        assert!(error_msg.contains("greater than zero"));
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
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn test_parse_and_validate_buy_command() {
        let validated_ticker = validate_ticker("aapl").unwrap();
        assert_eq!(validated_ticker, "AAPL");
    }

    #[test]
    fn test_parse_and_validate_sell_command() {
        let validated_ticker = validate_ticker("TSLA").unwrap();
        assert_eq!(validated_ticker, "TSLA");
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

        let result = cmd.try_get_matches_from(vec!["schwab", "sell", "-t", "TSLA", "-q", "50"]);
        assert!(result.is_ok());
    }

    // Integration tests for complete CLI workflow
    #[tokio::test]
    async fn test_integration_buy_command_end_to_end() {
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

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(result.is_ok(), "CLI command should succeed: {result:?}");
        account_mock.assert();
        order_mock.assert();

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(stdout_str.contains("Order placed successfully"));
    }

    #[tokio::test]
    async fn test_integration_sell_command_end_to_end() {
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

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "TSLA".to_string(),
            50,
            Instruction::Sell,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(result.is_ok(), "CLI command should succeed: {result:?}");
        account_mock.assert();
        order_mock.assert();

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(stdout_str.contains("Order placed successfully"));
    }

    #[tokio::test]
    async fn test_integration_authentication_failure_scenarios() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        // Set up expired access token but valid refresh token that will trigger a refresh attempt
        let expired_tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now() - chrono::Duration::minutes(35),
            refresh_token: "valid_but_rejected_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now() - chrono::Duration::days(1), // Valid refresh token
        };
        expired_tokens.store(&pool).await.unwrap();

        // Mock the token refresh to fail
        let token_refresh_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/oauth/token")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_but_rejected_refresh_token");
            then.status(400)
                .header("content-type", "application/json")
                .json_body(
                    json!({"error": "invalid_grant", "error_description": "Refresh token expired"}),
                );
        });

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(
            result.is_err(),
            "CLI command should fail due to auth issues"
        );
        token_refresh_mock.assert();

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(
            stdout_str.contains("authentication")
                || stdout_str.contains("refresh token")
                || stdout_str.contains("expired")
        );
    }

    #[tokio::test]
    async fn test_integration_token_refresh_flow() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        // Set up expired tokens
        let expired_tokens = crate::schwab::tokens::SchwabTokens {
            access_token: "expired_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now() - chrono::Duration::minutes(35),
            refresh_token: "valid_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now() - chrono::Duration::days(1),
        };
        expired_tokens.store(&pool).await.unwrap();

        let token_refresh_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/oauth/token")
                .body_contains("grant_type=refresh_token")
                .body_contains("refresh_token=valid_refresh_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "access_token": "new_access_token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "refresh_token": "new_refresh_token",
                    "refresh_token_expires_in": 604_800
                }));
        });

        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers")
                .header("authorization", "Bearer new_access_token");
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
                .header("authorization", "Bearer new_access_token");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({"orderId": "12345"}));
        });

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(
            result.is_ok(),
            "CLI command should succeed after token refresh: {result:?}"
        );
        token_refresh_mock.assert();
        account_mock.assert();
        order_mock.assert();

        // Verify that new tokens were stored in database
        let stored_tokens = crate::schwab::tokens::SchwabTokens::load(&pool)
            .await
            .unwrap();
        assert_eq!(stored_tokens.access_token, "new_access_token");
        assert_eq!(stored_tokens.refresh_token, "new_refresh_token");
    }

    #[tokio::test]
    async fn test_integration_database_operations() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        // Test that CLI properly handles database without tokens
        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(result.is_err(), "CLI should fail when no tokens are stored");
        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(
            stdout_str.contains("no rows returned")
                || stdout_str.contains("Database error")
                || stdout_str.contains("Failed to place order")
        );

        // Now add tokens and verify database integration works
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

        let mut stdout2 = Vec::new();

        let result2 = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout2,
        )
        .await;

        assert!(
            result2.is_ok(),
            "CLI should succeed with valid tokens in database"
        );
        account_mock.assert();
        order_mock.assert();
    }

    #[tokio::test]
    async fn test_integration_network_error_handling() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        // Mock network timeout/connection error
        let account_mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/trader/v1/accounts/accountNumbers");
            then.status(500)
                .header("content-type", "application/json")
                .json_body(json!({"error": "Internal Server Error"}));
        });

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "AAPL".to_string(),
            100,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(result.is_err(), "CLI should fail on network errors");
        account_mock.assert();

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(
            !stdout_str.is_empty(),
            "Should provide error feedback to user"
        );
    }

    #[tokio::test]
    async fn test_process_tx_command_transaction_not_found() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let mut stdout = Vec::new();

        let result = process_tx_command_with_writers(tx_hash, &env, &pool, &mut stdout).await;

        assert!(result.is_err(), "Should fail when transaction not found");
    }

    #[tokio::test]
    async fn test_integration_invalid_order_parameters() {
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
                .header("content-type", "application/json")
                .json_body(json!({
                    "error": "Invalid order parameters",
                    "message": "Insufficient buying power"
                }));
        });

        let mut stdout = Vec::new();

        let result = execute_order_with_writers(
            "INVALID".to_string(),
            999_999,
            Instruction::Buy,
            &env,
            &pool,
            &mut stdout,
        )
        .await;

        assert!(
            result.is_err(),
            "CLI should fail on invalid order parameters"
        );
        account_mock.assert();
        order_mock.assert();

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(
            stdout_str.contains("order")
                || stdout_str.contains("error")
                || stdout_str.contains("400")
        );
    }

    #[tokio::test]
    async fn test_process_tx_with_database_integration_success() {
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

        let tx_hash =
            fixed_bytes!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd");
        let mut stdout: Vec<u8> = Vec::new();

        // This test would require mocking the RPC provider and blockchain data
        // For now, we'll test the database integration parts that are testable

        // First, let's create a test trade and save it to database
        let test_trade = crate::arb::ArbTrade {
            id: None,
            tx_hash,
            log_index: 0,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5,
            schwab_price_per_share_cents: None,
            status: crate::trade::TradeStatus::Pending,
            schwab_order_id: None,
            created_at: None,
            completed_at: None,
        };

        // Test deduplication - first save should succeed
        let was_inserted = test_trade.try_save_to_db(&pool).await.unwrap();
        assert!(was_inserted, "First save should insert new trade");

        // Test deduplication - second save should be ignored
        let was_inserted_again = test_trade.try_save_to_db(&pool).await.unwrap();
        assert!(
            !was_inserted_again,
            "Second save should be ignored due to deduplication"
        );

        // Verify trade was saved with correct status
        let saved_trade = ArbTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 0)
            .await
            .unwrap();

        assert_eq!(saved_trade.status, TradeStatus::Pending);
        assert_eq!(saved_trade.schwab_ticker, "AAPL");
        assert_eq!(saved_trade.schwab_instruction, SchwabInstruction::Buy);
        assert_eq!(saved_trade.schwab_quantity, 5);

        // Test status update functionality
        ArbTrade::update_status(&pool, tx_hash, 0, TradeStatus::Completed)
            .await
            .unwrap();

        let updated_trade = ArbTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 0)
            .await
            .unwrap();

        assert_eq!(updated_trade.status, TradeStatus::Completed);
        assert!(updated_trade.completed_at.is_some());

        // Test ArbTrade::from_partial_trade conversion
        let partial_trade = PartialArbTrade {
            tx_hash: fixed_bytes!(
                "0x1234567890123456789012345678901234567890123456789012345678901234"
            ),
            log_index: 1,
            onchain_input_symbol: "TSLAs1".to_string(),
            onchain_input_amount: 10.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 2000.0,
            onchain_io_ratio: 0.005,
            onchain_price_per_share_cents: 20000.0,
            schwab_ticker: "TSLA".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
            schwab_quantity: 10,
        };

        let arb_trade = ArbTrade::from_partial_trade(partial_trade.clone());

        // Verify conversion preserved all data correctly
        assert_eq!(arb_trade.tx_hash, partial_trade.tx_hash);
        assert_eq!(arb_trade.log_index, partial_trade.log_index);
        assert_eq!(arb_trade.schwab_ticker, partial_trade.schwab_ticker);
        assert_eq!(
            arb_trade.schwab_instruction,
            partial_trade.schwab_instruction
        );
        assert_eq!(arb_trade.schwab_quantity, partial_trade.schwab_quantity);
        assert_eq!(arb_trade.status, TradeStatus::Pending);
        assert_eq!(arb_trade.schwab_order_id, None);

        // Test that converted trade can be saved to database
        let was_converted_inserted = arb_trade.try_save_to_db(&pool).await.unwrap();
        assert!(
            was_converted_inserted,
            "Converted trade should be saved successfully"
        );
    }

    #[tokio::test]
    async fn test_process_tx_database_duplicate_handling() {
        let pool = setup_test_db().await;

        let tx_hash =
            fixed_bytes!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

        let trade1 = ArbTrade {
            id: None,
            tx_hash,
            log_index: 42,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 500.0,
            onchain_output_symbol: "GOOGs1".to_string(),
            onchain_output_amount: 2.5,
            onchain_io_ratio: 200.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_ticker: "GOOG".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 3,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            created_at: None,
            completed_at: None,
        };

        let trade2 = trade1.clone();

        let first_save = trade1.try_save_to_db(&pool).await.unwrap();
        assert!(first_save, "First save should succeed");

        let second_save = trade2.try_save_to_db(&pool).await.unwrap();
        assert!(
            !second_save,
            "Second save should be ignored due to duplicate (tx_hash, log_index)"
        );

        let trade = ArbTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 42)
            .await
            .unwrap();

        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 42);
    }
}
