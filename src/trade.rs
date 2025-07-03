use alloy::primitives::Address;
use alloy::primitives::ruint::FromUintError;
use alloy::providers::Provider;
use lazy_static::lazy_static;
use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::bindings::IERC20::IERC20Instance;
use crate::bindings::IOrderBookV4::{TakeOrderConfigV3, TakeOrderV2};

pub(crate) struct Trade {
    #[allow(dead_code)] // TODO: remove this once we store trades in db
    onchain_input_symbol: String,
}

lazy_static! {
    static ref SYMBOL_MAP: RwLock<BTreeMap<Address, String>> = RwLock::new(BTreeMap::new());
}

#[derive(Debug, thiserror::Error)]
pub enum TradeConversionError {
    #[error("Invalid input index: {0}")]
    InvalidInputIndex(#[from] FromUintError<usize>),
    #[error("No input found at index: {0}")]
    NoInputAtIndex(usize),
    #[error("Failed to get symbol: {0}")]
    GetSymbol(#[from] alloy::contract::Error),
    #[error("Failed to acquire symbol map lock")]
    SymbolMapLock,
}

impl Trade {
    pub(crate) async fn try_from_take_order<P: Provider>(
        provider: P,
        event: TakeOrderV2,
    ) -> Result<Self, TradeConversionError> {
        let TakeOrderConfigV3 {
            order,
            inputIOIndex,
            outputIOIndex: _,
            signedContext: _,
        } = event.config;

        let input_index = usize::try_from(inputIOIndex)?;
        let input = order
            .validInputs
            .get(input_index)
            .ok_or(TradeConversionError::NoInputAtIndex(input_index))?;

        let maybe_symbol = {
            let read_guard = SYMBOL_MAP
                .read()
                .map_err(|_| TradeConversionError::SymbolMapLock)?;
            read_guard.get(&input.token).cloned()
        };

        let input_symbol = if let Some(symbol) = maybe_symbol {
            symbol
        } else {
            let erc20 = IERC20Instance::new(input.token, provider);
            let symbol = erc20.symbol().call().await?;

            let mut write_guard = SYMBOL_MAP
                .write()
                .map_err(|_| TradeConversionError::SymbolMapLock)?;
            write_guard.insert(input.token, symbol.clone());

            symbol
        };

        Ok(Trade {
            onchain_input_symbol: input_symbol,
        })
    }
}
