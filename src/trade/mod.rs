use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{B256, U256};
use std::num::ParseFloatError;

mod take_order;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchwabInstruction {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Trade {
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    tx_hash: B256,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    log_index: u64,

    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_input_symbol: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_input_amount: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_output_symbol: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_output_amount: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_io_ratio: f64,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_price_per_share_cents: u64,

    #[allow(dead_code)] // TODO: remove this once we store trades in db
    schwab_ticker: String,
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    schwab_instruction: SchwabInstruction,
}

#[derive(Debug, thiserror::Error)]
pub enum TradeConversionError {
    #[error("No transaction hash found in log")]
    NoTxHash,
    #[error("No log index found in log")]
    NoLogIndex,
    #[error("Invalid IO index: {0}")]
    InvalidIndex(#[from] FromUintError<usize>),
    #[error("No input found at index: {0}")]
    NoInputAtIndex(usize),
    #[error("No output found at index: {0}")]
    NoOutputAtIndex(usize),
    #[error("Failed to get symbol: {0}")]
    GetSymbol(#[from] alloy::contract::Error),
    #[error("Failed to acquire symbol map lock")]
    SymbolMapLock,
    #[error(
        "Invalid symbol configuration. Expected one USDC and one s1-suffixed symbol but got {0} and {1}"
    )]
    InvalidSymbolConfiguration(String, String),
    #[error("Failed to convert U256 to f64: {0}")]
    U256ToF64(#[from] ParseFloatError),
}

/// Helper that converts a fixed‐decimal `U256` amount into an `f64` using
/// the provided number of decimals.
///
/// NOTE: Parsing should never fail but precision may be lost.
fn u256_to_f64(amount: U256, decimals: u8) -> Result<f64, ParseFloatError> {
    if amount.is_zero() {
        return Ok(0.);
    }

    let u256_str = amount.to_string();
    let decimals = decimals as usize;

    let formatted = if decimals == 0 {
        u256_str
    } else if u256_str.len() <= decimals {
        format!("0.{}{}", "0".repeat(decimals - u256_str.len()), u256_str)
    } else {
        let (int_part, frac_part) = u256_str.split_at(u256_str.len() - decimals);
        format!("{int_part}.{frac_part}")
    };

    formatted.parse::<f64>()
}
