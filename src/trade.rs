use std::num::ParseFloatError;

use alloy::primitives::ruint::FromUintError;
use alloy::primitives::{B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;

use crate::bindings::IOrderBookV4::{TakeOrderConfigV3, TakeOrderV2};
use crate::symbol_cache::SymbolCache;

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

    // #[allow(dead_code)] // TODO: remove this once we store trades in db
    // onchain_io_ratio: U256,
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

impl Trade {
    pub(crate) async fn try_from_take_order<P: Provider>(
        cache: &SymbolCache,
        provider: P,
        event: TakeOrderV2,
        log: Log,
    ) -> Result<Self, TradeConversionError> {
        let TakeOrderV2 {
            config,
            input: input_amount,
            output: output_amount,
            sender: _,
        } = event;

        let TakeOrderConfigV3 {
            order,
            inputIOIndex,
            outputIOIndex,
            signedContext: _,
        } = config;

        let tx_hash = log.transaction_hash.ok_or(TradeConversionError::NoTxHash)?;
        let log_index = log.log_index.ok_or(TradeConversionError::NoLogIndex)?;

        let input_index = usize::try_from(inputIOIndex)?;
        let input = order
            .validInputs
            .get(input_index)
            .ok_or(TradeConversionError::NoInputAtIndex(input_index))?;

        let output_index = usize::try_from(outputIOIndex)?;
        let output = order
            .validOutputs
            .get(output_index)
            .ok_or(TradeConversionError::NoOutputAtIndex(output_index))?;

        let input_decimals = input.decimals;
        let onchain_input_amount = u256_to_f64(input_amount, input_decimals)?;

        let output_decimals = output.decimals;
        let onchain_output_amount = u256_to_f64(output_amount, output_decimals)?;

        let onchain_input_symbol = cache.get_io_symbol(&provider, input).await?;
        let onchain_output_symbol = cache.get_io_symbol(provider, output).await?;

        let (schwab_ticker, schwab_instruction) =
            if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
                let ticker = onchain_output_symbol
                    .strip_suffix("s1")
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Sell)
            } else if onchain_output_symbol == "USDC" && onchain_input_symbol.ends_with("s1") {
                let ticker = onchain_input_symbol
                    .strip_suffix("s1")
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        TradeConversionError::InvalidSymbolConfiguration(
                            onchain_input_symbol.clone(),
                            onchain_output_symbol.clone(),
                        )
                    })?;
                (ticker, SchwabInstruction::Buy)
            } else {
                return Err(TradeConversionError::InvalidSymbolConfiguration(
                    onchain_input_symbol,
                    onchain_output_symbol,
                ));
            };

        let trade = Trade {
            tx_hash,
            log_index,

            onchain_input_symbol,
            onchain_input_amount,
            onchain_output_symbol,
            onchain_output_amount,

            schwab_ticker,
            schwab_instruction,
        };

        Ok(trade)
    }
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

#[cfg(test)]
mod tests {
    use alloy::primitives::{LogData, U256, address, bytes, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::SolCall;
    use std::str::FromStr;

    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::{EvaluableV3, IO, OrderV3};

    #[tokio::test]
    async fn test_try_from_take_order_err() {
        let asserter = Asserter::new();
        asserter.push_failure_msg("reverted");

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(true);
        let cache = SymbolCache::default();

        let error = Trade::try_from_take_order(&cache, &provider, take_order.clone(), get_log())
            .await
            .unwrap_err();

        assert!(
            matches!(error, TradeConversionError::GetSymbol(_)),
            "unexpected error: {error:?}",
        );

        let mut no_tx_hash_log = get_log();
        no_tx_hash_log.transaction_hash = None;

        let error =
            Trade::try_from_take_order(&cache, &provider, take_order.clone(), no_tx_hash_log)
                .await
                .unwrap_err();

        assert!(matches!(error, TradeConversionError::NoTxHash));

        let mut no_log_index_log = get_log();
        no_log_index_log.log_index = None;

        let error = Trade::try_from_take_order(&cache, &provider, take_order, no_log_index_log)
            .await
            .unwrap_err();

        assert!(matches!(error, TradeConversionError::NoLogIndex));
    }

    #[tokio::test]
    async fn test_try_from_take_order_ok_sell_schwab() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(true);
        let cache = SymbolCache::default();

        let trade = Trade::try_from_take_order(&cache, &provider, take_order.clone(), get_log())
            .await
            .unwrap();

        let expected_trade = Trade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 100.0,
            onchain_output_symbol: "FOOs1".to_string(),
            onchain_output_amount: 9.0,
            schwab_ticker: "FOO".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
        };

        assert_eq!(trade, expected_trade);

        // test that the symbol is cached
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let trade = Trade::try_from_take_order(&cache, &provider, take_order, get_log())
            .await
            .unwrap();

        assert_eq!(trade.onchain_input_symbol, "USDC");
        assert_eq!(trade.onchain_output_symbol, "FOOs1");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Sell);
        assert_eq!(trade.schwab_ticker, "FOO");
    }

    #[tokio::test]
    async fn test_try_from_take_order_ok_schwab_buy() {
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(false);
        let cache = SymbolCache::default();

        let trade = Trade::try_from_take_order(&cache, &provider, take_order.clone(), get_log())
            .await
            .unwrap();

        let expected_trade = Trade {
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 293,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 9.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 100.0,
            schwab_ticker: "BAR".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
        };

        assert_eq!(trade, expected_trade);

        // test that the symbol is cached
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let trade = Trade::try_from_take_order(&cache, &provider, take_order, get_log())
            .await
            .unwrap();

        assert_eq!(trade.onchain_input_symbol, "BARs1");
        assert_eq!(trade.onchain_output_symbol, "USDC");
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_symbol_decode() {
        // Simulate provider returning an error for symbol decoding
        let asserter = Asserter::new();
        // Push a failure for input symbol
        asserter.push_failure_msg("decode error");
        // Push a success for output symbol (should not be called)
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(true);
        let log = get_log();
        let cache = SymbolCache::default();

        let result = Trade::try_from_take_order(&cache, &provider, take_order, log).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_output_symbol_decode() {
        // Simulate provider returning an error for output symbol decoding
        let asserter = Asserter::new();
        // Push a success for input symbol
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));
        // Push a failure for output symbol
        asserter.push_failure_msg("decode error");

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(false);
        let log = get_log();
        let cache = SymbolCache::default();

        let result = Trade::try_from_take_order(&cache, &provider, take_order, log).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_missing_tx_hash() {
        // Test with a log missing transaction_hash
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(true);
        let mut log = get_log();
        log.transaction_hash = None;
        let cache = SymbolCache::default();

        let result = Trade::try_from_take_order(&cache, &provider, take_order, log).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_missing_log_index() {
        // Test with a log missing log_index
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(false);
        let mut log = get_log();
        log.log_index = None;
        let cache = SymbolCache::default();

        let result = Trade::try_from_take_order(&cache, &provider, take_order, log).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_invalid_symbol_configuration_usdc_usdc() {
        // Both input and output symbols are "USDC"
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let take_order = get_take_order_event(true);
        let cache = SymbolCache::default();

        let err = Trade::try_from_take_order(&cache, &provider, take_order, get_log())
            .await
            .unwrap_err();
        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "USDC" && output == "USDC"),
            "Expected InvalidSymbolConfiguration with USDC/USDC, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_invalid_symbol_configuration_no_s1_suffix() {
        // Input is "USDC", output is "FOO" (no s1 suffix)
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOO".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err =
            Trade::try_from_take_order(&cache, &provider, get_take_order_event(false), get_log())
                .await
                .unwrap_err();
        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "USDC" && output == "FOO"),
            "Expected InvalidSymbolConfiguration with USDC/FOO, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_invalid_symbol_configuration_both_s1() {
        // Both input and output have s1 suffix
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err =
            Trade::try_from_take_order(&cache, &provider, get_take_order_event(true), get_log())
                .await
                .unwrap_err();
        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "FOOs1" && output == "BARs1"),
            "Expected InvalidSymbolConfiguration with FOOs1/BARs1, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_try_from_take_order_err_invalid_symbol_configuration_output_usdc_input_no_s1() {
        // Output is "USDC", input is "FOO" (no s1 suffix)
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOO".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err =
            Trade::try_from_take_order(&cache, &provider, get_take_order_event(false), get_log())
                .await
                .unwrap_err();
        assert!(
            matches!(err, TradeConversionError::InvalidSymbolConfiguration(ref input, ref output) if input == "FOO" && output == "USDC"),
            "Expected InvalidSymbolConfiguration with FOO/USDC, got: {err:?}"
        );
    }

    fn get_take_order_event(buy: bool) -> TakeOrderV2 {
        TakeOrderV2 {
            sender: address!("0x0000000000000000000000000000000000000000"),
            config: TakeOrderConfigV3 {
                order: OrderV3 {
                    owner: address!("0x1111111111111111111111111111111111111111"),
                    evaluable: EvaluableV3 {
                        interpreter: address!("0x2222222222222222222222222222222222222222"),
                        store: address!("0x3333333333333333333333333333333333333333"),
                        bytecode: bytes!("0x00"),
                    },
                    nonce: fixed_bytes!(
                        "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                    ),
                    validInputs: vec![
                        IO {
                            token: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                            decimals: 6,
                            vaultId: U256::from(0),
                        },
                        IO {
                            token: address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                            decimals: 18,
                            vaultId: U256::from(0),
                        },
                    ],
                    validOutputs: vec![
                        IO {
                            token: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                            decimals: 6,
                            vaultId: U256::from(0),
                        },
                        IO {
                            token: address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                            decimals: 18,
                            vaultId: U256::from(0),
                        },
                    ],
                },
                inputIOIndex: if buy { U256::from(0) } else { U256::from(1) },
                outputIOIndex: if buy { U256::from(1) } else { U256::from(0) },
                signedContext: vec![],
            },
            // 100 bucks or 9 shares
            input: if buy {
                U256::from(100000000)
            } else {
                U256::from_str("9000000000000000000").unwrap()
            },
            // 9 shares or 100 bucks
            output: if buy {
                U256::from_str("9000000000000000000").unwrap()
            } else {
                U256::from(100000000)
            },
        }
    }

    fn get_log() -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: address!("0xfefefefefefefefefefefefefefefefefefefefe"),
                data: LogData::empty(),
            },
            block_hash: None,
            block_number: None,
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )),
            transaction_index: None,
            log_index: Some(293),
            removed: false,
        }
    }
}
