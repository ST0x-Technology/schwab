use crate::bindings::IOrderBookV4::{EvaluableV3, IO, OrderV3};
use alloy::primitives::{LogData, U256, address, bytes, fixed_bytes};
use alloy::rpc::types::Log;

/// Returns a test `OrderV3` instance that is shared across multiple
/// unit-tests. The exact values are not important – only that the
/// structure is valid and deterministic.
#[must_use]
pub fn get_test_order() -> OrderV3 {
    OrderV3 {
        owner: address!("0x1111111111111111111111111111111111111111"),
        evaluable: EvaluableV3 {
            interpreter: address!("0x2222222222222222222222222222222222222222"),
            store: address!("0x3333333333333333333333333333333333333333"),
            bytecode: bytes!("0x00"),
        },
        nonce: fixed_bytes!("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        validInputs: vec![
            IO {
                token: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                decimals: 6, // USDC-like token
                vaultId: U256::from(0),
            },
            IO {
                token: address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                decimals: 18, // Stock share token
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
    }
}

/// Creates a generic `Log` stub with the supplied log index. This helper is
/// useful when the concrete value of most fields is irrelevant for the
/// assertion being performed.
#[must_use]
pub fn create_log(log_index: u64) -> Log {
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
        log_index: Some(log_index),
        removed: false,
    }
}

/// Convenience wrapper that returns the log routinely used by the
/// higher-level tests in `trade::mod` (with log index set to `293`).
#[must_use]
pub fn get_test_log() -> Log {
    create_log(293)
}
