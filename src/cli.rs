use clap::{Parser, Subcommand};
use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use super::*;

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
