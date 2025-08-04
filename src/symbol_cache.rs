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
    #[cfg(test)]
    pub fn insert_for_test(&self, address: Address, symbol: String) {
        self.map.write().unwrap().insert(address, symbol);
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, address};
    use alloy::providers::{ProviderBuilder, mock::Asserter};

    #[tokio::test]
    async fn test_symbol_cache_hit() {
        let cache = SymbolCache::default();
        let address = address!("0x1234567890123456789012345678901234567890");

        cache
            .map
            .write()
            .unwrap()
            .insert(address, "TEST".to_string());

        let io = IO {
            token: address,
            decimals: 18,
            vaultId: U256::from(0),
        };

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let result = cache.get_io_symbol(provider, &io).await.unwrap();
        assert_eq!(result, "TEST");
    }

    #[tokio::test]
    async fn test_symbol_cache_miss_rpc_failure() {
        let cache = SymbolCache::default();
        let address = address!("0x1234567890123456789012345678901234567890");

        let io = IO {
            token: address,
            decimals: 18,
            vaultId: U256::from(0),
        };

        let asserter = Asserter::new();
        asserter.push_failure_msg("RPC failure");
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let result = cache.get_io_symbol(provider, &io).await;
        assert!(matches!(
            result.unwrap_err(),
            TradeConversionError::GetSymbol(_)
        ));
    }
}
