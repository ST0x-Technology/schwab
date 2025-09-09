//! Order I/O handling logic for processing symbol pairs, amounts, and trade details.
//! This module centralizes all logic related to parsing and validating onchain order data.

use std::fmt;
use std::str::FromStr;

use crate::error::{OnChainError, TradeValidationError};
use crate::schwab::Direction;

/// Macro to create a TokenizedEquitySymbol at compile time, similar to alloy's address! macro.
/// This macro validates the symbol format at compile time.
#[macro_export]
macro_rules! tokenized_symbol {
    ($symbol:literal) => {{
        // Validate at compile time by attempting to parse
        const _: () = {
            #[allow(clippy::string_lit_as_bytes)]
            let bytes = $symbol.as_bytes();
            let len = bytes.len();

            // Check minimum length (at least 3 chars: 1 for ticker + 2 for suffix)
            if len < 3 {
                panic!(concat!(
                    "Invalid tokenized equity symbol: '",
                    $symbol,
                    "' (too short)"
                ));
            }

            // Check for valid suffixes
            let has_0x = len >= 2 && bytes[len - 2] == b'0' && bytes[len - 1] == b'x';
            let has_s1 = len >= 2 && bytes[len - 2] == b's' && bytes[len - 1] == b'1';

            if !has_0x && !has_s1 {
                panic!(concat!(
                    "Invalid tokenized equity symbol: '",
                    $symbol,
                    "' (must end with '0x' or 's1')"
                ));
            }

            // Check that the base symbol isn't empty
            if len == 2 {
                panic!(concat!(
                    "Invalid tokenized equity symbol: '",
                    $symbol,
                    "' (missing base symbol)"
                ));
            }
        };

        // At runtime, we can safely unwrap since we validated at compile time
        crate::onchain::io::TokenizedEquitySymbol::parse($symbol).unwrap()
    }};
}

// The macro is available via the crate::tokenized_symbol path

/// Macro to create an EquitySymbol at compile time.
/// This macro validates the symbol format at compile time.
#[macro_export]
macro_rules! symbol {
    ($symbol:literal) => {{
        // Validate at compile time by attempting to parse
        const _: () = {
            #[allow(clippy::string_lit_as_bytes)]
            let bytes = $symbol.as_bytes();
            let len = bytes.len();

            // Check minimum length
            if len == 0 {
                panic!(concat!(
                    "Invalid equity symbol: '",
                    $symbol,
                    "' (cannot be empty)"
                ));
            }

            // Check maximum length
            if len > 32 {
                panic!(concat!(
                    "Invalid equity symbol: '",
                    $symbol,
                    "' (too long, max 32 characters)"
                ));
            }

            // Check for whitespace
            if $symbol.chars().any(|c| c.is_whitespace()) {
                panic!(concat!(
                    "Invalid equity symbol: '",
                    $symbol,
                    "' (cannot contain whitespace)"
                ));
            }

            // Check for USDC
            if $symbol == "USDC" {
                panic!(concat!(
                    "Invalid equity symbol: '",
                    $symbol,
                    "' (USDC is not an equity symbol)"
                ));
            }
        };

        // At runtime, we can safely unwrap since we validated at compile time
        EquitySymbol::new($symbol).unwrap()
    }};
}

/// Represents a validated number of shares (non-negative)
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shares(f64);

impl Shares {
    pub(crate) fn new(value: f64) -> Result<Self, TradeValidationError> {
        if value < 0.0 {
            return Err(TradeValidationError::NegativeShares(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn value(self) -> f64 {
        self.0
    }
}

/// Represents a validated USDC amount (non-negative)
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Usdc(f64);

impl Usdc {
    pub(crate) fn new(value: f64) -> Result<Self, TradeValidationError> {
        if value < 0.0 {
            return Err(TradeValidationError::NegativeUsdc(value));
        }
        Ok(Self(value))
    }

    pub(crate) fn value(self) -> f64 {
        self.0
    }
}

/// Represents a validated base equity symbol (e.g., "AAPL", "MSFT")
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EquitySymbol(String);

impl EquitySymbol {
    /// Creates a new EquitySymbol with validation
    pub(crate) fn new(symbol: &str) -> Result<Self, OnChainError> {
        // Reject USDC as it's not an equity
        if symbol == "USDC" {
            return Err(OnChainError::Validation(
                TradeValidationError::NotTokenizedEquity(symbol.to_string()),
            ));
        }

        // Basic validation - no whitespace, reasonable length, not empty
        if symbol.chars().any(char::is_whitespace) || symbol.len() > 32 || symbol.is_empty() {
            return Err(OnChainError::Validation(
                TradeValidationError::NotTokenizedEquity(symbol.to_string()),
            ));
        }

        Ok(Self(symbol.to_string()))
    }

    /// Gets the base symbol string
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EquitySymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for EquitySymbol {
    type Err = OnChainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// The suffix for tokenized equity symbols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenizedEquitySuffix {
    ZeroX, // "0x"
    S1,    // "s1"
}

impl TokenizedEquitySuffix {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroX => "0x",
            Self::S1 => "s1",
        }
    }
}

impl fmt::Display for TokenizedEquitySuffix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Represents a validated tokenized equity symbol with guaranteed format
/// Composed of a base equity symbol and a tokenized suffix
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenizedEquitySymbol {
    base: EquitySymbol,
    suffix: TokenizedEquitySuffix,
}

impl TokenizedEquitySymbol {
    /// Creates a new TokenizedEquitySymbol from components
    pub(crate) const fn new(base: EquitySymbol, suffix: TokenizedEquitySuffix) -> Self {
        Self { base, suffix }
    }

    /// Creates a new TokenizedEquitySymbol from a string (e.g., "AAPL0x")
    pub(crate) fn parse(symbol: &str) -> Result<Self, OnChainError> {
        // Try to extract suffix first
        if let Some(stripped) = symbol.strip_suffix("0x") {
            let base = EquitySymbol::new(stripped)?;
            return Ok(Self::new(base, TokenizedEquitySuffix::ZeroX));
        }

        if let Some(stripped) = symbol.strip_suffix("s1") {
            let base = EquitySymbol::new(stripped)?;
            return Ok(Self::new(base, TokenizedEquitySuffix::S1));
        }

        // No valid suffix found
        Err(OnChainError::Validation(
            TradeValidationError::NotTokenizedEquity(symbol.to_string()),
        ))
    }

    /// Gets the base equity symbol
    pub(crate) const fn base(&self) -> &EquitySymbol {
        &self.base
    }

    /// Extract the base symbol (equivalent to the old extract_base_from_tokenized)
    pub(crate) fn extract_base(&self) -> String {
        self.base.as_str().to_string()
    }
}

impl fmt::Display for TokenizedEquitySymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.base, self.suffix)
    }
}

impl FromStr for TokenizedEquitySymbol {
    type Err = OnChainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Trade details extracted from symbol pair processing
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TradeDetails {
    ticker: EquitySymbol,
    equity_amount: Shares,
    usdc_amount: Usdc,
    direction: Direction,
}

impl TradeDetails {
    /// Gets the ticker symbol
    #[cfg(test)]
    pub(crate) fn ticker(&self) -> &EquitySymbol {
        &self.ticker
    }

    /// Gets the equity amount
    pub(crate) fn equity_amount(&self) -> Shares {
        self.equity_amount
    }

    /// Gets the USDC amount
    pub(crate) fn usdc_amount(&self) -> Usdc {
        self.usdc_amount
    }

    /// Gets the trade direction
    pub(crate) fn direction(&self) -> Direction {
        self.direction
    }
    /// Extracts trade details from input/output symbol and amount pairs
    pub(crate) fn try_from_io(
        input_symbol: &str,
        input_amount: f64,
        output_symbol: &str,
        output_amount: f64,
    ) -> Result<Self, OnChainError> {
        // Determine direction and ticker using existing logic
        let (ticker, direction) = determine_schwab_trade_details(input_symbol, output_symbol)?;

        // Extract equity and USDC amounts based on which symbol is the tokenized equity
        let (equity_amount_raw, usdc_amount_raw) = if input_symbol == "USDC"
            && TokenizedEquitySymbol::parse(output_symbol).is_ok()
        {
            // USDC → tokenized equity: output is equity, input is USDC
            (output_amount, input_amount)
        } else if output_symbol == "USDC" && TokenizedEquitySymbol::parse(input_symbol).is_ok() {
            // tokenized equity → USDC: input is equity, output is USDC
            (input_amount, output_amount)
        } else {
            // This should not happen if determine_schwab_trade_details passed, but be defensive
            return Err(TradeValidationError::InvalidSymbolConfiguration(
                input_symbol.to_string(),
                output_symbol.to_string(),
            )
            .into());
        };

        // Validate amounts using newtype constructors
        let equity_amount = Shares::new(equity_amount_raw)?;
        let usdc_amount = Usdc::new(usdc_amount_raw)?;

        Ok(Self {
            ticker,
            equity_amount,
            usdc_amount,
            direction,
        })
    }
}

/// Determines onchain trade direction and ticker based on onchain symbol configuration.
///
/// If the on-chain order has USDC as input and a tokenized stock (0x or s1 suffix) as
/// output then it means the order received USDC and gave away a tokenized stock,
/// i.e. sold the tokenized stock onchain.
fn determine_schwab_trade_details(
    onchain_input_symbol: &str,
    onchain_output_symbol: &str,
) -> Result<(EquitySymbol, Direction), OnChainError> {
    // USDC input + tokenized stock output = sold tokenized stock onchain
    if onchain_input_symbol == "USDC" {
        if let Ok(tokenized) = TokenizedEquitySymbol::parse(onchain_output_symbol) {
            return Ok((tokenized.base().clone(), Direction::Sell));
        }
    }

    // tokenized stock input + USDC output = bought tokenized stock onchain
    if onchain_output_symbol == "USDC" {
        if let Ok(tokenized) = TokenizedEquitySymbol::parse(onchain_input_symbol) {
            return Ok((tokenized.base().clone(), Direction::Buy));
        }
    }

    Err(TradeValidationError::InvalidSymbolConfiguration(
        onchain_input_symbol.to_string(),
        onchain_output_symbol.to_string(),
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenized_equity_symbol_parse() {
        // Test that TokenizedEquitySymbol::parse correctly identifies tokenized symbols
        assert!(TokenizedEquitySymbol::parse("AAPL0x").is_ok());
        assert!(TokenizedEquitySymbol::parse("NVDAs1").is_ok());
        assert!(TokenizedEquitySymbol::parse("GME0x").is_ok());
        assert!(TokenizedEquitySymbol::parse("USDC").is_err());
        assert!(TokenizedEquitySymbol::parse("AAPL").is_err());
        assert!(TokenizedEquitySymbol::parse("").is_err());
    }

    #[test]
    fn test_tokenized_equity_symbol_extract_base() {
        // Test the extract_base method (replaces extract_base_from_tokenized)
        let symbol = TokenizedEquitySymbol::parse("AAPL0x").unwrap();
        assert_eq!(symbol.extract_base(), "AAPL");

        let symbol = TokenizedEquitySymbol::parse("NVDAs1").unwrap();
        assert_eq!(symbol.extract_base(), "NVDA");

        let symbol = TokenizedEquitySymbol::parse("GME0x").unwrap();
        assert_eq!(symbol.extract_base(), "GME");

        // Test edge cases - suffix-only symbols should be invalid
        let error = TokenizedEquitySymbol::parse("0x").unwrap_err();
        assert!(matches!(
            error,
            OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s.is_empty()
        ));

        let error = TokenizedEquitySymbol::parse("s1").unwrap_err();
        assert!(matches!(
            error,
            OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s.is_empty()
        ));
    }

    #[test]
    fn test_tokenized_equity_symbol_valid() {
        let symbol = TokenizedEquitySymbol::parse("AAPL0x").unwrap();
        assert_eq!(symbol.to_string(), "AAPL0x");
        assert_eq!(symbol.extract_base(), "AAPL");

        let symbol = TokenizedEquitySymbol::parse("NVDAs1").unwrap();
        assert_eq!(symbol.to_string(), "NVDAs1");
        assert_eq!(symbol.extract_base(), "NVDA");
    }

    #[test]
    fn test_tokenized_equity_symbol_invalid() {
        // Empty symbol
        let error = TokenizedEquitySymbol::parse("").unwrap_err();
        assert!(matches!(
            error,
            OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s.is_empty()
        ));

        // USDC symbol
        let error = TokenizedEquitySymbol::parse("USDC").unwrap_err();
        assert!(matches!(
            error,
            OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s == "USDC"
        ));

        // Non-tokenized equity symbols
        let error = TokenizedEquitySymbol::parse("AAPL").unwrap_err();
        assert!(
            matches!(error, OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s == "AAPL")
        );

        let error = TokenizedEquitySymbol::parse("INVALID").unwrap_err();
        assert!(
            matches!(error, OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s == "INVALID")
        );

        let error = TokenizedEquitySymbol::parse("MSFT").unwrap_err();
        assert!(
            matches!(error, OnChainError::Validation(TradeValidationError::NotTokenizedEquity(ref s)) if s == "MSFT")
        );
    }

    #[test]
    fn test_shares_validation() {
        // Test valid shares
        let shares = Shares::new(100.5).unwrap();
        assert!((shares.value() - 100.5).abs() < f64::EPSILON);

        // Test zero shares (valid)
        let shares = Shares::new(0.0).unwrap();
        assert!((shares.value() - 0.0).abs() < f64::EPSILON);

        // Test negative shares (invalid)
        let result = Shares::new(-1.0);
        assert!(matches!(
            result.unwrap_err(),
            TradeValidationError::NegativeShares(-1.0)
        ));
    }

    #[test]
    fn test_usdc_validation() {
        // Test valid USDC amount
        let usdc = Usdc::new(1000.50).unwrap();
        assert!((usdc.value() - 1000.50).abs() < f64::EPSILON);

        // Test zero USDC (valid)
        let usdc = Usdc::new(0.0).unwrap();
        assert!((usdc.value() - 0.0).abs() < f64::EPSILON);

        // Test negative USDC (invalid)
        let result = Usdc::new(-100.0);
        assert!(matches!(
            result.unwrap_err(),
            TradeValidationError::NegativeUsdc(-100.0)
        ));
    }

    #[test]
    fn test_shares_usdc_equality() {
        let shares1 = Shares::new(100.0).unwrap();
        let shares2 = Shares::new(100.0).unwrap();
        let shares3 = Shares::new(200.0).unwrap();

        assert_eq!(shares1, shares2);
        assert_ne!(shares1, shares3);

        let usdc1 = Usdc::new(1000.0).unwrap();
        let usdc2 = Usdc::new(1000.0).unwrap();
        let usdc3 = Usdc::new(2000.0).unwrap();

        assert_eq!(usdc1, usdc2);
        assert_ne!(usdc1, usdc3);
    }

    #[test]
    fn test_determine_schwab_trade_details_usdc_to_0x() {
        let result = determine_schwab_trade_details("USDC", "AAPL0x").unwrap();
        assert_eq!(result.0, "AAPL");
        assert_eq!(result.1, Direction::Sell); // Onchain sold AAPL0x for USDC

        let result = determine_schwab_trade_details("USDC", "TSLA0x").unwrap();
        assert_eq!(result.0, "TSLA");
        assert_eq!(result.1, Direction::Sell); // Onchain sold TSLA0x for USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_usdc_to_s1() {
        let result = determine_schwab_trade_details("USDC", "NVDAs1").unwrap();
        assert_eq!(result.0, "NVDA");
        assert_eq!(result.1, Direction::Sell); // Onchain sold NVDAs1 for USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_0x_to_usdc() {
        let result = determine_schwab_trade_details("AAPL0x", "USDC").unwrap();
        assert_eq!(result.0, "AAPL");
        assert_eq!(result.1, Direction::Buy); // Onchain bought AAPL0x with USDC

        let result = determine_schwab_trade_details("TSLA0x", "USDC").unwrap();
        assert_eq!(result.0, "TSLA");
        assert_eq!(result.1, Direction::Buy); // Onchain bought TSLA0x with USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_s1_to_usdc() {
        let result = determine_schwab_trade_details("NVDAs1", "USDC").unwrap();
        assert_eq!(result.0, "NVDA");
        assert_eq!(result.1, Direction::Buy); // Onchain bought NVDAs1 with USDC
    }

    #[test]
    fn test_determine_schwab_trade_details_invalid_configurations() {
        let result = determine_schwab_trade_details("BTC", "ETH");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("USDC", "USDC");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("AAPL0x", "TSLA0x");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = determine_schwab_trade_details("", "");
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));
    }

    #[test]
    fn test_trade_details_try_from_io_usdc_to_0x_equity() {
        let details = TradeDetails::try_from_io("USDC", 100.0, "AAPL0x", 0.5).unwrap();

        assert_eq!(details.ticker(), &symbol!("AAPL"));
        assert!((details.equity_amount().value() - 0.5).abs() < f64::EPSILON);
        assert!((details.usdc_amount().value() - 100.0).abs() < f64::EPSILON);
        assert_eq!(details.direction(), Direction::Sell);
    }

    #[test]
    fn test_trade_details_try_from_io_usdc_to_s1_equity_fixes_bug() {
        // This is the key test - s1 suffix should work correctly now
        let details = TradeDetails::try_from_io("USDC", 64.17, "NVDAs1", 0.374).unwrap();

        assert_eq!(details.ticker(), &symbol!("NVDA"));
        assert!((details.equity_amount().value() - 0.374).abs() < f64::EPSILON); // Should be 0.374, not 64.17!
        assert!((details.usdc_amount().value() - 64.17).abs() < f64::EPSILON);
        assert_eq!(details.direction(), Direction::Sell);
    }

    #[test]
    fn test_trade_details_try_from_io_0x_equity_to_usdc() {
        let details = TradeDetails::try_from_io("AAPL0x", 0.5, "USDC", 100.0).unwrap();

        assert_eq!(details.ticker(), &symbol!("AAPL"));
        assert!((details.equity_amount().value() - 0.5).abs() < f64::EPSILON);
        assert!((details.usdc_amount().value() - 100.0).abs() < f64::EPSILON);
        assert_eq!(details.direction(), Direction::Buy);
    }

    #[test]
    fn test_trade_details_try_from_io_s1_equity_to_usdc() {
        let details = TradeDetails::try_from_io("NVDAs1", 0.374, "USDC", 64.17).unwrap();

        assert_eq!(details.ticker(), &symbol!("NVDA"));
        assert!((details.equity_amount().value() - 0.374).abs() < f64::EPSILON);
        assert!((details.usdc_amount().value() - 64.17).abs() < f64::EPSILON);
        assert_eq!(details.direction(), Direction::Buy);
    }

    #[test]
    fn test_trade_details_try_from_io_invalid_configurations() {
        let result = TradeDetails::try_from_io("USDC", 100.0, "USDC", 100.0);
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));

        let result = TradeDetails::try_from_io("BTC", 1.0, "ETH", 3000.0);
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::InvalidSymbolConfiguration(_, _))
        ));
    }

    #[test]
    fn test_trade_details_negative_amount_validation() {
        // Test negative equity amount
        let result = TradeDetails::try_from_io("USDC", 100.0, "AAPL0x", -0.5);
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::NegativeShares(_))
        ));

        // Test negative USDC amount
        let result = TradeDetails::try_from_io("USDC", -100.0, "AAPL0x", 0.5);
        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(TradeValidationError::NegativeUsdc(_))
        ));
    }

    #[test]
    fn test_tokenized_symbol_macro() {
        // Test that the macro creates valid symbols
        let aapl_symbol = tokenized_symbol!("AAPL0x");
        assert_eq!(aapl_symbol.to_string(), "AAPL0x");
        assert_eq!(aapl_symbol.base().as_str(), "AAPL");

        let nvda_symbol = tokenized_symbol!("NVDAs1");
        assert_eq!(nvda_symbol.to_string(), "NVDAs1");
        assert_eq!(nvda_symbol.base().as_str(), "NVDA");

        // Test that compile-time validation works (these should compile)
        let _valid_symbols = [
            tokenized_symbol!("MSFT0x"),
            tokenized_symbol!("GOOGs1"),
            tokenized_symbol!("TSLA0x"),
        ];
    }

    #[test]
    fn test_symbol_macro() {
        // Test that the macro creates valid symbols
        let aapl_symbol = symbol!("AAPL");
        assert_eq!(aapl_symbol.as_str(), "AAPL");

        let nvda_symbol = symbol!("NVDA");
        assert_eq!(nvda_symbol.as_str(), "NVDA");

        // Test that compile-time validation works (these should compile)
        let _valid_symbols = [symbol!("MSFT"), symbol!("GOOG"), symbol!("TSLA")];
    }
}
