use alloy::{primitives::Address, providers::Provider};
use backon::{ExponentialBuilder, Retryable};
use std::{collections::BTreeMap, sync::RwLock};

use crate::bindings::{IERC20::IERC20Instance, IOrderBookV4::IO};
use crate::trade::TradeConversionError;

#[derive(Debug, Default)]
pub struct SymbolCache {
    map: RwLock<BTreeMap<Address, String>>,
}

impl SymbolCache {
    pub async fn get_io_symbol<P: Provider>(
        &self,
        provider: P,
        io: &IO,
    ) -> Result<String, TradeConversionError> {
        let maybe_symbol = {
            let read_guard = self
                .map
                .read()
                .map_err(|_| TradeConversionError::SymbolMapLock)?;
            read_guard.get(&io.token).cloned()
        };

        if let Some(symbol) = maybe_symbol {
            return Ok(symbol);
        }

        let erc20 = IERC20Instance::new(io.token, provider);
        let symbol = (|| async { erc20.symbol().call().await })
            .retry(ExponentialBuilder::new().with_max_times(3))
            .await?;

        self.map
            .write()
            .map_err(|_| TradeConversionError::SymbolMapLock)?
            .insert(io.token, symbol.clone());

        Ok(symbol)
    }
}
