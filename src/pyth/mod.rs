use alloy::primitives::{Address, B256, Bytes, address};
use alloy::providers::Provider;
use alloy::providers::ext::DebugApi;
use alloy::rpc::types::trace::geth::{
    CallFrame, GethDebugBuiltInTracerType, GethDebugTracerType, GethDebugTracingOptions, GethTrace,
};
use alloy::sol_types::{SolCall, SolType};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use tracing::{debug, error, info, warn};

use crate::bindings::IPyth::{
    getEmaPriceNoOlderThanCall, getEmaPriceUnsafeCall, getPriceNoOlderThanCall, getPriceUnsafeCall,
};
use crate::bindings::PythStructs::Price;

mod feed_id_cache;
pub use feed_id_cache::FeedIdCache;

pub const BASE_PYTH_CONTRACT_ADDRESS: Address =
    address!("0x8250f4aF4B972684F7b336503E2D6dFeDeB1487a");

#[derive(Debug, thiserror::Error)]
pub enum PythError {
    #[error("No Pyth oracle call found in transaction trace")]
    NoPythCall,
    #[error("No Pyth call found matching price feed ID {0}")]
    NoMatchingFeedId(B256),
    #[error("Failed to decode Pyth return data: {0}")]
    DecodeError(String),
    #[error("Pyth response structure invalid: {0}")]
    InvalidResponse(String),
    #[error("Trace is not CallTracer variant")]
    InvalidTraceVariant,
    #[error("Arithmetic overflow in price conversion")]
    ArithmeticOverflow,
    #[error("RPC error while fetching trace: {0}")]
    RpcError(String),
}

#[derive(Debug, Clone)]
pub struct PythCall {
    pub price_feed_id: B256,
    pub output: Bytes,
    pub depth: u32,
}

pub fn find_pyth_calls(trace: &GethTrace) -> Result<Vec<PythCall>, PythError> {
    match trace {
        GethTrace::CallTracer(call_frame) => Ok(traverse_call_frame(call_frame, 0)),
        _ => Err(PythError::InvalidTraceVariant),
    }
}

fn traverse_call_frame(frame: &CallFrame, depth: u32) -> Vec<PythCall> {
    let current_call = frame
        .to
        .filter(|&to| to == BASE_PYTH_CONTRACT_ADDRESS)
        .filter(|_| is_pyth_method_selector(&frame.input))
        .and(frame.output.as_ref())
        .and_then(|output| {
            extract_price_feed_id(&frame.input).map(|feed_id| PythCall {
                price_feed_id: feed_id,
                output: output.clone(),
                depth,
            })
        });

    let nested_calls = frame
        .calls
        .iter()
        .flat_map(|nested_call| traverse_call_frame(nested_call, depth + 1));

    current_call.into_iter().chain(nested_calls).collect()
}

fn is_pyth_method_selector(input: &Bytes) -> bool {
    if input.len() < 4 {
        return false;
    }

    let selector = &input[0..4];

    selector == getPriceNoOlderThanCall::SELECTOR
        || selector == getPriceUnsafeCall::SELECTOR
        || selector == getEmaPriceNoOlderThanCall::SELECTOR
        || selector == getEmaPriceUnsafeCall::SELECTOR
}

fn extract_price_feed_id(input: &Bytes) -> Option<B256> {
    if input.len() < 36 {
        return None;
    }

    let feed_id_bytes = &input[4..36];
    Some(B256::from_slice(feed_id_bytes))
}

pub fn decode_pyth_price(output: &Bytes) -> Result<Price, PythError> {
    let price = Price::abi_decode(output)
        .map_err(|e| PythError::DecodeError(format!("ABI decode failed: {e}")))?;

    Ok(price)
}

pub fn to_decimal(price: &Price) -> Result<Decimal, PythError> {
    let exponent = price.expo;

    let result = if exponent >= 0 {
        let price_value = Decimal::from_i64(price.price)
            .ok_or_else(|| PythError::InvalidResponse("price value too large".to_string()))?;

        let multiplier = (0..exponent).try_fold(Decimal::from(1_i64), |acc, _| {
            acc.checked_mul(Decimal::from(10_i64))
                .ok_or(PythError::ArithmeticOverflow)
        })?;

        price_value
            .checked_mul(multiplier)
            .ok_or(PythError::ArithmeticOverflow)
    } else {
        let decimals = exponent
            .checked_abs()
            .ok_or(PythError::ArithmeticOverflow)?
            .try_into()
            .map_err(|_| PythError::InvalidResponse("exponent too large".to_string()))?;

        Decimal::try_new(price.price, decimals)
            .map_err(|e| PythError::InvalidResponse(format!("failed to create decimal: {e}")))
    }?;

    Ok(result.normalize())
}

pub async fn extract_pyth_price<P>(
    tx_hash: B256,
    provider: &P,
    symbol: &str,
    cache: &FeedIdCache,
) -> Result<Price, PythError>
where
    P: Provider,
{
    debug!("Fetching trace for tx {tx_hash}");

    let trace = fetch_transaction_trace(tx_hash, provider).await?;

    debug!("Parsing trace for Pyth oracle calls");

    let pyth_calls = find_pyth_calls(&trace)?;

    if pyth_calls.is_empty() {
        warn!("No Pyth call found in transaction {tx_hash}");
        return Err(PythError::NoPythCall);
    }

    debug!("Found {} Pyth call(s) in trace", pyth_calls.len());

    let cached_feed_id = cache.get(symbol).await;

    let matching_call = if let Some(feed_id) = cached_feed_id {
        debug!("Found cached feed ID for {symbol}: {feed_id}");

        pyth_calls
            .iter()
            .find(|call| call.price_feed_id == feed_id)
            .ok_or_else(|| {
                warn!(
                    "No Pyth call found matching cached feed ID {feed_id} for {symbol} in transaction {tx_hash}"
                );
                PythError::NoMatchingFeedId(feed_id)
            })?
    } else {
        debug!("No cached feed ID for {symbol}, using first Pyth call and caching");

        let first_call = &pyth_calls[0];
        cache
            .insert(symbol.to_string(), first_call.price_feed_id)
            .await;

        info!(
            "Cached new feed ID mapping: {symbol} -> {}",
            first_call.price_feed_id
        );

        first_call
    };

    debug!(
        "Using Pyth call at depth {} with feed ID {} for price extraction",
        matching_call.depth, matching_call.price_feed_id
    );

    let price = decode_pyth_price(&matching_call.output).map_err(|e| {
        error!("Failed to extract Pyth price from {tx_hash}: {e}");
        e
    })?;

    info!(
        "Extracted Pyth price for {symbol} (feed {}): {} (expo: {}, conf: {})",
        matching_call.price_feed_id, price.price, price.expo, price.conf
    );

    Ok(price)
}

async fn fetch_transaction_trace<P>(tx_hash: B256, provider: &P) -> Result<GethTrace, PythError>
where
    P: Provider,
{
    let options = GethDebugTracingOptions {
        tracer: Some(GethDebugTracerType::BuiltInTracer(
            GethDebugBuiltInTracerType::CallTracer,
        )),
        ..Default::default()
    };

    let trace = provider
        .debug_trace_transaction(tx_hash, options)
        .await
        .map_err(|e| PythError::RpcError(e.to_string()))?;

    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, U256};

    fn create_test_call_frame(
        to: Address,
        input: Vec<u8>,
        output: Option<Vec<u8>>,
        nested_calls: Vec<CallFrame>,
    ) -> CallFrame {
        CallFrame {
            from: Address::ZERO,
            gas: U256::from(100_000u64),
            gas_used: U256::from(50_000u64),
            to: Some(to),
            input: Bytes::from(input),
            output: output.map(Bytes::from),
            error: None,
            revert_reason: None,
            calls: nested_calls,
            logs: vec![],
            value: Some(U256::ZERO),
            typ: "CALL".to_string(),
        }
    }

    #[test]
    fn test_find_pyth_calls_single_call_at_root() {
        let pyth_selector = crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR;
        let mut input = pyth_selector.to_vec();
        let feed_id = B256::repeat_byte(0xaa);
        input.extend_from_slice(feed_id.as_slice());

        let output = vec![0x01, 0x02, 0x03, 0x04];

        let call_frame = create_test_call_frame(
            BASE_PYTH_CONTRACT_ADDRESS,
            input,
            Some(output.clone()),
            vec![],
        );

        let trace = GethTrace::CallTracer(call_frame);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].price_feed_id, feed_id);
        assert_eq!(result[0].output.as_ref(), &output);
        assert_eq!(result[0].depth, 0);
    }

    #[test]
    fn test_find_pyth_calls_nested() {
        let pyth_selector = crate::bindings::IPyth::getPriceUnsafeCall::SELECTOR;
        let mut input = pyth_selector.to_vec();
        let feed_id = B256::repeat_byte(0xbb);
        input.extend_from_slice(feed_id.as_slice());

        let output = vec![0xaa, 0xbb, 0xcc];

        let nested_pyth_call = create_test_call_frame(
            BASE_PYTH_CONTRACT_ADDRESS,
            input,
            Some(output.clone()),
            vec![],
        );

        let root_call = create_test_call_frame(
            Address::repeat_byte(0x11),
            vec![0x01, 0x02, 0x03, 0x04],
            Some(vec![0x05, 0x06]),
            vec![nested_pyth_call],
        );

        let trace = GethTrace::CallTracer(root_call);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].price_feed_id, feed_id);
        assert_eq!(result[0].output.as_ref(), &output);
        assert_eq!(result[0].depth, 1);
    }

    #[test]
    fn test_find_pyth_calls_multiple() {
        let pyth_selector1 = crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR;
        let pyth_selector2 = crate::bindings::IPyth::getEmaPriceNoOlderThanCall::SELECTOR;

        let mut input1 = pyth_selector1.to_vec();
        let feed_id1 = B256::repeat_byte(0xcc);
        input1.extend_from_slice(feed_id1.as_slice());

        let mut input2 = pyth_selector2.to_vec();
        let feed_id2 = B256::repeat_byte(0xdd);
        input2.extend_from_slice(feed_id2.as_slice());

        let output1 = vec![0x01, 0x02];
        let output2 = vec![0x03, 0x04];

        let pyth_call1 = create_test_call_frame(
            BASE_PYTH_CONTRACT_ADDRESS,
            input1,
            Some(output1.clone()),
            vec![],
        );
        let pyth_call2 = create_test_call_frame(
            BASE_PYTH_CONTRACT_ADDRESS,
            input2,
            Some(output2.clone()),
            vec![],
        );

        let root_call = create_test_call_frame(
            Address::repeat_byte(0x11),
            vec![0x01, 0x02],
            Some(vec![0x05]),
            vec![pyth_call1, pyth_call2],
        );

        let trace = GethTrace::CallTracer(root_call);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].price_feed_id, feed_id1);
        assert_eq!(result[0].output.as_ref(), &output1);
        assert_eq!(result[0].depth, 1);
        assert_eq!(result[1].price_feed_id, feed_id2);
        assert_eq!(result[1].output.as_ref(), &output2);
        assert_eq!(result[1].depth, 1);
    }

    #[test]
    fn test_find_pyth_calls_no_pyth_calls() {
        let call_frame = create_test_call_frame(
            Address::repeat_byte(0x11),
            vec![0x01, 0x02, 0x03, 0x04],
            Some(vec![0x05, 0x06]),
            vec![],
        );

        let trace = GethTrace::CallTracer(call_frame);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_find_pyth_calls_wrong_selector() {
        let wrong_selector = vec![0xff, 0xff, 0xff, 0xff];
        let mut input = wrong_selector;
        input.extend_from_slice(&[0u8; 32]);

        let call_frame =
            create_test_call_frame(BASE_PYTH_CONTRACT_ADDRESS, input, Some(vec![0x01]), vec![]);

        let trace = GethTrace::CallTracer(call_frame);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_find_pyth_calls_no_output() {
        let pyth_selector = crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR;
        let mut input = pyth_selector.to_vec();
        input.extend_from_slice(&[0u8; 32]);

        let call_frame = create_test_call_frame(BASE_PYTH_CONTRACT_ADDRESS, input, None, vec![]);

        let trace = GethTrace::CallTracer(call_frame);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_find_pyth_calls_deeply_nested() {
        let pyth_selector = crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR;
        let mut input = pyth_selector.to_vec();
        let feed_id = B256::repeat_byte(0xee);
        input.extend_from_slice(feed_id.as_slice());

        let output = vec![0xde, 0xad];

        let level_3_pyth = create_test_call_frame(
            BASE_PYTH_CONTRACT_ADDRESS,
            input,
            Some(output.clone()),
            vec![],
        );

        let level_2 = create_test_call_frame(
            Address::repeat_byte(0x22),
            vec![0x01],
            Some(vec![0x02]),
            vec![level_3_pyth],
        );

        let level_1 = create_test_call_frame(
            Address::repeat_byte(0x11),
            vec![0x03],
            Some(vec![0x04]),
            vec![level_2],
        );

        let trace = GethTrace::CallTracer(level_1);
        let result = find_pyth_calls(&trace).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].price_feed_id, feed_id);
        assert_eq!(result[0].output.as_ref(), &output);
        assert_eq!(result[0].depth, 2);
    }

    #[test]
    fn test_is_pyth_method_selector_valid_selectors() {
        let selectors = [
            crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR,
            crate::bindings::IPyth::getPriceUnsafeCall::SELECTOR,
            crate::bindings::IPyth::getEmaPriceNoOlderThanCall::SELECTOR,
            crate::bindings::IPyth::getEmaPriceUnsafeCall::SELECTOR,
        ];

        for selector in selectors {
            let mut input = selector.to_vec();
            let feed_id = B256::repeat_byte(0xff);
            input.extend_from_slice(feed_id.as_slice());

            assert!(
                is_pyth_method_selector(&Bytes::from(input)),
                "Selector {selector:?} should be recognized"
            );
        }
    }

    #[test]
    fn test_is_pyth_method_selector_invalid() {
        let invalid_input = Bytes::from(vec![0xff, 0xff, 0xff, 0xff]);
        assert!(!is_pyth_method_selector(&invalid_input));

        let short_input = Bytes::from(vec![0x01, 0x02]);
        assert!(!is_pyth_method_selector(&short_input));

        let empty_input = Bytes::from(vec![]);
        assert!(!is_pyth_method_selector(&empty_input));
    }

    #[test]
    fn test_decode_pyth_price_valid() {
        let price = Price {
            price: 100_000,
            conf: 500,
            expo: -5,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let encoded = Price::abi_encode(&price);
        let encoded_bytes = Bytes::from(encoded);

        let decoded = decode_pyth_price(&encoded_bytes).unwrap();

        assert_eq!(decoded.price, 100_000);
        assert_eq!(decoded.conf, 500);
        assert_eq!(decoded.expo, -5);
        assert_eq!(
            decoded.publishTime,
            alloy::primitives::U256::from(1_700_000_000u64)
        );
    }

    #[test]
    fn test_decode_pyth_price_malformed() {
        let malformed = Bytes::from(vec![0x01, 0x02, 0x03]);
        let result = decode_pyth_price(&malformed);

        assert!(matches!(result, Err(PythError::DecodeError(_))));
    }

    #[test]
    fn test_to_decimal_negative_exponent() {
        let price = Price {
            price: 123_456_789,
            conf: 1000,
            expo: -8,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let decimal = to_decimal(&price).unwrap();

        assert_eq!(decimal.to_string(), "1.23456789");
    }

    #[test]
    fn test_to_decimal_zero_exponent() {
        let price = Price {
            price: 42,
            conf: 1,
            expo: 0,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let decimal = to_decimal(&price).unwrap();

        assert_eq!(decimal.to_string(), "42");
    }

    #[test]
    fn test_to_decimal_positive_exponent() {
        let price = Price {
            price: 123,
            conf: 10,
            expo: 3,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let decimal = to_decimal(&price).unwrap();

        assert_eq!(decimal.to_string(), "123000");
    }

    #[test]
    fn test_to_decimal_various_exponents() {
        let test_cases = vec![
            (100_000_000, -6, "100"),
            (1_500_000, -6, "1.5"),
            (500, -2, "5"),
            (42_000, -3, "42"),
            (1, 0, "1"),
            (1, 5, "100000"),
        ];

        for (price_value, expo, expected) in test_cases {
            let price = Price {
                price: price_value,
                conf: 100,
                expo,
                publishTime: alloy::primitives::U256::from(1_700_000_000u64),
            };

            let decimal = to_decimal(&price).unwrap();

            assert_eq!(
                decimal.to_string(),
                expected,
                "Failed for price={price_value}, expo={expo}"
            );
        }
    }

    #[test]
    fn test_to_decimal_negative_price() {
        let price = Price {
            price: -50_000_000,
            conf: 1000,
            expo: -6,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let decimal = to_decimal(&price).unwrap();

        assert_eq!(decimal.to_string(), "-50");
    }

    #[test]
    fn test_to_decimal_equity_price() {
        let price = Price {
            price: 18_250,
            conf: 10,
            expo: -2,
            publishTime: alloy::primitives::U256::from(1_700_000_000u64),
        };

        let decimal = to_decimal(&price).unwrap();

        assert_eq!(decimal.to_string(), "182.5");
    }

    #[test]
    fn test_decode_and_convert_roundtrip() {
        let original_price = Price {
            price: 999_999_999,
            conf: 5000,
            expo: -8,
            publishTime: alloy::primitives::U256::from(1_700_123_456u64),
        };

        let encoded = Price::abi_encode(&original_price);
        let encoded_bytes = Bytes::from(encoded);

        let decoded = decode_pyth_price(&encoded_bytes).unwrap();
        let decimal = to_decimal(&decoded).unwrap();

        assert_eq!(decoded.price, original_price.price);
        assert_eq!(decoded.conf, original_price.conf);
        assert_eq!(decoded.expo, original_price.expo);
        assert_eq!(decoded.publishTime, original_price.publishTime);
        assert_eq!(decimal.to_string(), "9.99999999");
    }
}
