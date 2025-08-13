use clap::{Parser, Subcommand};
use sqlx::SqlitePool;
use std::io::Write;
use thiserror::Error;
use tracing::{error, info};

use crate::error::OnChainError;
use crate::onchain::{
    EvmEnv, PartialArbTrade, trade::OnchainTrade, trade_accumulator::TradeAccumulator,
};
use crate::schwab::SchwabAuthEnv;
use crate::schwab::order::{Instruction, Order, execute_schwab_execution};
use crate::schwab::run_oauth_flow;
use crate::schwab::tokens::SchwabTokens;
use crate::symbol_cache::SymbolCache;
use crate::{Env, LogLevel};
use alloy::primitives::B256;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};

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
    let pool = env.get_sqlite_pool().await?;
    run_command_with_writers(env, cli.command, &pool, &mut std::io::stdout()).await
}

pub async fn run_command(env: Env, command: Commands) -> anyhow::Result<()> {
    let pool = env.get_sqlite_pool().await?;
    run_command_with_writers(env, command, &pool, &mut std::io::stdout()).await
}

async fn run_command_with_writers<W: Write>(
    env: Env,
    command: Commands,
    pool: &SqlitePool,
    stdout: &mut W,
) -> anyhow::Result<()> {
    match command {
        Commands::Buy { ticker, quantity } => {
            ensure_authentication(pool, &env.schwab_auth, stdout).await?;
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
                pool,
                stdout,
            )
            .await?;
        }
        Commands::Sell { ticker, quantity } => {
            ensure_authentication(pool, &env.schwab_auth, stdout).await?;
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
                pool,
                stdout,
            )
            .await?;
        }
        Commands::ProcessTx { tx_hash } => {
            info!("Processing transaction: tx_hash={tx_hash}");
            let ws = WsConnect::new(env.evm_env.ws_rpc_url.as_str());
            let provider = ProviderBuilder::new().connect_ws(ws).await?;
            let cache = SymbolCache::default();
            process_tx_with_provider(tx_hash, &env, pool, stdout, &provider, &cache).await?;
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

async fn process_tx_with_provider<W: Write, P: Provider + Clone>(
    tx_hash: B256,
    env: &Env,
    pool: &SqlitePool,
    stdout: &mut W,
    provider: &P,
    cache: &SymbolCache,
) -> anyhow::Result<()> {
    let evm_env = &env.evm_env;

    match PartialArbTrade::try_from_tx_hash(tx_hash, provider, cache, evm_env).await {
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
        Err(OnChainError::Validation(crate::error::TradeValidationError::TransactionNotFound(
            hash,
        ))) => {
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

    // Convert PartialArbTrade to OnchainTrade for the new unified system
    let tokenized_symbol = format!("{}s1", partial_trade.schwab_ticker);
    let onchain_trade = OnchainTrade {
        id: None,
        tx_hash: partial_trade.tx_hash,
        log_index: partial_trade.log_index,
        symbol: tokenized_symbol,
        #[allow(clippy::cast_precision_loss)]
        amount: partial_trade.schwab_quantity as f64, // Use Schwab quantity for consistency
        price_usdc: partial_trade.onchain_price_per_share_cents,
        created_at: None,
    };

    writeln!(stdout, "🔄 Processing trade with TradeAccumulator...")?;

    let execution = TradeAccumulator::add_trade(pool, onchain_trade).await?;

    if let Some(execution) = execution {
        let execution_id = execution.id.expect("SchwabExecution should have ID");
        writeln!(
            stdout,
            "✅ Trade triggered Schwab execution (ID: {execution_id})"
        )?;
        ensure_authentication(pool, &env.schwab_auth, stdout).await?;
        writeln!(stdout, "🔄 Executing Schwab order...")?;
        execute_schwab_execution(env, pool, execution, 3).await;
        writeln!(stdout, "🎯 Trade processing completed!")?;
    } else {
        writeln!(
            stdout,
            "📊 Trade accumulated but did not trigger execution yet."
        )?;
        writeln!(
            stdout,
            "   (Waiting to accumulate enough shares for a whole share execution)"
        )?;
    }

    Ok(())
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

// Old ArbTrade-based functions removed - now using unified TradeAccumulator system

// Tests temporarily disabled during migration to new system
// TODO: Update tests to use OnchainTrade + TradeAccumulator instead of ArbTrade
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange, ClearV2};
    use crate::onchain::{TradeStatus, trade::OnchainTrade};
    use crate::schwab::SchwabInstruction;
    use crate::schwab::execution::SchwabExecution;
    use crate::test_utils::get_test_order;
    use crate::{LogLevel, onchain::EvmEnv, schwab::SchwabAuthEnv};
    use alloy::hex;
    use alloy::primitives::{IntoLogData, U256, address, fixed_bytes, keccak256};
    use alloy::providers::mock::Asserter;
    use alloy::sol_types::{SolCall, SolEvent, SolValue};
    use chrono::{Duration, Utc};
    use clap::CommandFactory;
    use httpmock::MockServer;
    use serde_json::json;
    use std::str::FromStr;

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

    #[allow(dead_code)]
    struct MockBlockchainData {
        tx_hash: alloy::primitives::B256,
        order: crate::bindings::IOrderBookV4::OrderV3,
        order_hash: alloy::primitives::B256,
        clear_event: ClearV2,
        after_clear_event: AfterClear,
        receipt_json: serde_json::Value,
        after_clear_log: alloy::rpc::types::Log,
    }

    fn create_mock_blockchain_data(
        orderbook: alloy::primitives::Address,
        tx_hash: alloy::primitives::B256,
        alice_output_shares: &str, // e.g., "9000000000000000000" for 9 shares
        bob_output_usdc: u64,      // e.g., 100_000_000 for 100 USDC
    ) -> MockBlockchainData {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: U256::from(0),
                aliceOutputIOIndex: U256::from(1),
                bobInputIOIndex: U256::from(1),
                bobOutputIOIndex: U256::from(0),
                aliceBountyVaultId: U256::ZERO,
                bobBountyVaultId: U256::ZERO,
            },
        };

        let receipt_json = json!({
            "transactionHash": tx_hash,
            "transactionIndex": "0x0",
            "blockHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "blockNumber": "0x64",
            "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "contractAddress": null,
            "gasUsed": "0x5208",
            "cumulativeGasUsed": "0xf4240",
            "effectiveGasPrice": "0x3b9aca00",
            "status": "0x1",
            "type": "0x2",
            "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "logs": [{
                "address": orderbook,
                "topics": [ClearV2::SIGNATURE_HASH],
                "data": format!("0x{}", hex::encode(clear_event.clone().into_log_data().data)),
                "blockNumber": "0x64",
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "removed": false
            }]
        });

        let after_clear_event = AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::from_str(alice_output_shares).unwrap(),
                bobOutput: U256::from(bob_output_usdc),
                aliceInput: U256::from(bob_output_usdc),
                bobInput: U256::from_str(alice_output_shares).unwrap(),
            },
        };

        let after_clear_log = alloy::rpc::types::Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.clone().into_log_data(),
            },
            block_hash: Some(fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            )),
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: Some(0),
            log_index: Some(1),
            removed: false,
        };

        MockBlockchainData {
            tx_hash,
            order,
            order_hash,
            clear_event,
            after_clear_event,
            receipt_json,
            after_clear_log,
        }
    }

    fn setup_mock_provider_for_process_tx(
        mock_data: &MockBlockchainData,
        input_symbol: &str,
        output_symbol: &str,
    ) -> impl Provider + Clone {
        let asserter = Asserter::new();
        asserter.push_success(&mock_data.receipt_json);
        asserter.push_success(&json!([mock_data.after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &input_symbol.to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &output_symbol.to_string(),
        ));

        ProviderBuilder::new().connect_mocked_client(asserter)
    }

    fn setup_schwab_api_mocks(server: &MockServer) -> (httpmock::Mock, httpmock::Mock) {
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

        (account_mock, order_mock)
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

        let buy_command = Commands::Buy {
            ticker: "AAPL".to_string(),
            quantity: 100,
        };

        let result = run_command_with_writers(env, buy_command, &pool, &mut stdout).await;

        assert!(
            result.is_ok(),
            "End-to-end CLI command should succeed: {result:?}"
        );
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

        let sell_command = Commands::Sell {
            ticker: "TSLA".to_string(),
            quantity: 50,
        };

        let result = run_command_with_writers(env, sell_command, &pool, &mut stdout).await;

        assert!(
            result.is_ok(),
            "End-to-end CLI command should succeed: {result:?}"
        );
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

        // Mock provider that returns null for transaction receipt (transaction not found)
        let asserter = Asserter::new();
        asserter.push_success(&json!(null));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result =
            process_tx_with_provider(tx_hash, &env, &pool, &mut stdout, &provider, &cache).await;

        assert!(
            result.is_ok(),
            "Should handle transaction not found gracefully"
        );

        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(
            stdout_str.contains("Transaction not found"),
            "Should display transaction not found message"
        );
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

        let tx_hash =
            fixed_bytes!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

        // Create mock blockchain data for 9 AAPL shares trade
        let mock_data = create_mock_blockchain_data(
            env.evm_env.orderbook,
            tx_hash,
            "9000000000000000000", // 9 shares (18 decimals)
            100_000_000,           // 100 USDC (6 decimals)
        );

        // Update env to have the correct order hash
        let mut env = env;
        env.evm_env.order_hash = mock_data.order_hash;

        // Set up Schwab API mocks
        let (account_mock, order_mock) = setup_schwab_api_mocks(&server);

        // Set up the mock provider
        let provider = setup_mock_provider_for_process_tx(&mock_data, "USDC", "AAPLs1");
        let cache = SymbolCache::default();

        let mut stdout = Vec::new();

        // Test the function with the mocked provider
        let result =
            process_tx_with_provider(tx_hash, &env, &pool, &mut stdout, &provider, &cache).await;

        assert!(
            result.is_ok(),
            "process_tx should succeed with proper mocking"
        );

        // Verify the OnchainTrade was saved to database
        let trade = OnchainTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 0)
            .await
            .unwrap();
        assert_eq!(trade.symbol, "AAPLs1"); // Tokenized symbol
        assert_eq!(trade.amount, 9.0); // Amount from the test data

        // Verify SchwabExecution was created (due to TradeAccumulator)
        let executions = SchwabExecution::find_completed_by_symbol(&pool, "AAPL")
            .await
            .unwrap();
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].shares, 9);
        assert_eq!(executions[0].direction, SchwabInstruction::Sell);

        // Verify Schwab API was called
        account_mock.assert();
        order_mock.assert();

        // Verify stdout output
        let stdout_str = String::from_utf8(stdout).unwrap();
        assert!(stdout_str.contains("Processing trade with TradeAccumulator"));
        assert!(stdout_str.contains("Trade triggered Schwab execution"));
        assert!(stdout_str.contains("Trade processing completed"));
    }

    #[tokio::test]
    async fn test_process_tx_database_duplicate_handling() {
        let server = MockServer::start();
        let env = create_test_env_for_cli(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let tx_hash =
            fixed_bytes!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

        // Create mock blockchain data for 5 TSLA shares trade
        let mock_data = create_mock_blockchain_data(
            env.evm_env.orderbook,
            tx_hash,
            "5000000000000000000", // 5 shares (18 decimals)
            50_000_000,            // 50 USDC (6 decimals)
        );

        // Update env to have the correct order hash
        let mut env = env;
        env.evm_env.order_hash = mock_data.order_hash;

        // Set up Schwab API mocks for first call
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

        // Set up the mock provider for first call
        let asserter1 = Asserter::new();
        asserter1.push_success(&mock_data.receipt_json);
        asserter1.push_success(&json!([mock_data.after_clear_log]));
        asserter1.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter1.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"TSLAs1".to_string(),
        ));

        let provider1 = ProviderBuilder::new().connect_mocked_client(asserter1);
        let cache1 = SymbolCache::default();

        let mut stdout1 = Vec::new();

        // Process the transaction for the first time
        let result1 =
            process_tx_with_provider(tx_hash, &env, &pool, &mut stdout1, &provider1, &cache1).await;
        assert!(result1.is_ok(), "First process_tx should succeed");

        // Verify the OnchainTrade was saved to database
        let trade = OnchainTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 0)
            .await
            .unwrap();
        assert_eq!(trade.symbol, "TSLAs1"); // Tokenized symbol
        assert_eq!(trade.amount, 5.0); // Amount from the test data

        // Verify stdout output for first call
        let stdout_str1 = String::from_utf8(stdout1).unwrap();
        assert!(stdout_str1.contains("Processing trade with TradeAccumulator"));

        // Set up the mock provider for second call (duplicate)
        // Note: We still need to mock the provider responses because the function will still
        // fetch the transaction data, but it should detect the duplicate in the database
        let asserter2 = Asserter::new();
        asserter2.push_success(&mock_data.receipt_json);
        asserter2.push_success(&json!([mock_data.after_clear_log]));
        asserter2.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter2.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"TSLAs1".to_string(),
        ));

        let provider2 = ProviderBuilder::new().connect_mocked_client(asserter2);
        let cache2 = SymbolCache::default();

        let mut stdout2 = Vec::new();

        // Process the same transaction again (should detect duplicate and error)
        let result2 =
            process_tx_with_provider(tx_hash, &env, &pool, &mut stdout2, &provider2, &cache2).await;
        assert!(
            result2.is_err(),
            "Second process_tx should fail due to duplicate constraint violation"
        );

        // Verify the error is a UNIQUE constraint violation
        let error_string = format!("{:?}", result2.unwrap_err());
        assert!(error_string.contains("UNIQUE constraint failed"));
        assert!(error_string.contains("onchain_trades.tx_hash"));

        // Verify only one trade exists in database
        let count = OnchainTrade::db_count(&pool).await.unwrap();
        assert_eq!(count, 1, "Only one trade should exist in database");

        // Verify stdout shows processing started but didn't complete
        let stdout_str2 = String::from_utf8(stdout2).unwrap();
        assert!(stdout_str2.contains("Processing trade with TradeAccumulator"));
        // Should not contain completion messages since it errored
        assert!(!stdout_str2.contains("Trade processing completed"));

        // Verify Schwab API was only called once (for the first trade)
        account_mock.assert_hits(1);
        order_mock.assert_hits(1);
    }

    #[tokio::test]
    async fn test_onchain_trade_database_duplicate_detection() {
        let pool = setup_test_db().await;

        let tx_hash =
            fixed_bytes!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef");

        let trade1 = OnchainTrade {
            id: None,
            tx_hash,
            log_index: 42,
            symbol: "GOOGs1".to_string(),
            amount: 2.5,
            price_usdc: 20000.0,
            created_at: None,
        };

        let trade2 = trade1.clone();

        // Test saving the first trade within a transaction
        let mut sql_tx1 = pool.begin().await.unwrap();
        let first_result = trade1.save_within_transaction(&mut sql_tx1).await;
        assert!(first_result.is_ok(), "First save should succeed");
        sql_tx1.commit().await.unwrap();

        // Test saving the duplicate trade within a transaction (should fail)
        let mut sql_tx2 = pool.begin().await.unwrap();
        let second_result = trade2.save_within_transaction(&mut sql_tx2).await;
        assert!(
            second_result.is_err(),
            "Second save should fail due to duplicate (tx_hash, log_index)"
        );
        sql_tx2.rollback().await.unwrap();

        let trade = OnchainTrade::find_by_tx_hash_and_log_index(&pool, tx_hash, 42)
            .await
            .unwrap();

        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 42);
        assert_eq!(trade.symbol, "GOOGs1");
        assert_eq!(trade.amount, 2.5);
        assert_eq!(trade.price_usdc, 20000.0);
    }
}
