use alloy::primitives::Address;

pub const PYTH_CONTRACT_ADDRESS: Address =
    alloy::primitives::address!("4305FB66699C3B2702D4d05CF36551390A4c69C6");

#[derive(Debug, thiserror::Error)]
pub enum PythError {
    #[error("No Pyth oracle call found in transaction trace")]
    NoPythCall,

    #[error("Failed to decode Pyth return data: {0}")]
    DecodeError(String),

    #[error("Pyth response structure invalid: {0}")]
    InvalidResponse(String),
}
