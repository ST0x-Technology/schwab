use alloy::primitives::{Address, Bytes, address};
use alloy::rpc::types::trace::geth::{CallFrame, GethTrace};
use alloy::sol_types::SolCall;

use crate::bindings::IPyth::{
    getEmaPriceNoOlderThanCall, getEmaPriceUnsafeCall, getPriceNoOlderThanCall, getPriceUnsafeCall,
};

pub const BASE_PYTH_CONTRACT_ADDRESS: Address =
    address!("0x8250f4aF4B972684F7b336503E2D6dFeDeB1487a");

#[derive(Debug, thiserror::Error)]
pub enum PythError {
    #[error("No Pyth oracle call found in transaction trace")]
    NoPythCall,
    #[error("Failed to decode Pyth return data: {0}")]
    DecodeError(String),
    #[error("Pyth response structure invalid: {0}")]
    InvalidResponse(String),
    #[error("Trace is not CallTracer variant")]
    InvalidTraceVariant,
}

#[derive(Debug, Clone)]
pub struct PythCall {
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
        .map(|output| PythCall {
            output: output.clone(),
            depth,
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
        input.extend_from_slice(&[0u8; 32]);

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
        assert_eq!(result[0].output.as_ref(), &output);
        assert_eq!(result[0].depth, 0);
    }

    #[test]
    fn test_find_pyth_calls_nested() {
        let pyth_selector = crate::bindings::IPyth::getPriceUnsafeCall::SELECTOR;
        let mut input = pyth_selector.to_vec();
        input.extend_from_slice(&[0u8; 32]);

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
        assert_eq!(result[0].output.as_ref(), &output);
        assert_eq!(result[0].depth, 1);
    }

    #[test]
    fn test_find_pyth_calls_multiple() {
        let pyth_selector1 = crate::bindings::IPyth::getPriceNoOlderThanCall::SELECTOR;
        let pyth_selector2 = crate::bindings::IPyth::getEmaPriceNoOlderThanCall::SELECTOR;

        let mut input1 = pyth_selector1.to_vec();
        input1.extend_from_slice(&[0u8; 32]);

        let mut input2 = pyth_selector2.to_vec();
        input2.extend_from_slice(&[0u8; 32]);

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
        assert_eq!(result[0].output.as_ref(), &output1);
        assert_eq!(result[0].depth, 1);
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
        input.extend_from_slice(&[0u8; 32]);

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
            input.extend_from_slice(&[0u8; 32]);

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
}
