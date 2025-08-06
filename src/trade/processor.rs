use alloy::primitives::B256;
use alloy::providers::Provider;
use alloy::rpc::types::Log;

use super::{EvmEnv, PartialArbTrade, TradeConversionError};
use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::symbol_cache::SymbolCache;

impl PartialArbTrade {
    /// Attempts to create a `PartialArbTrade` from a transaction hash by looking up
    /// the transaction receipt and parsing relevant orderbook events.
    pub async fn try_from_tx_hash<P: Provider>(
        tx_hash: B256,
        provider: P,
        cache: &SymbolCache,
        env: &EvmEnv,
    ) -> Result<Option<Self>, TradeConversionError> {
        let receipt = provider
            .get_transaction_receipt(tx_hash)
            .await?
            .ok_or(TradeConversionError::TransactionNotFound(tx_hash))?;

        let logs = receipt.inner.logs();

        for (log_index, log) in logs.iter().enumerate() {
            if log.address() != env.orderbook {
                continue;
            }

            if let Ok(clear_event) = log.log_decode::<ClearV2>() {
                let log_with_metadata = Log {
                    inner: log.inner.clone(),
                    block_hash: receipt.block_hash,
                    block_number: receipt.block_number,
                    block_timestamp: None,
                    transaction_hash: Some(tx_hash),
                    transaction_index: receipt.transaction_index,
                    log_index: Some(log_index as u64),
                    removed: false,
                };

                if let Some(trade) = Self::try_from_clear_v2(
                    env,
                    cache,
                    &provider,
                    clear_event.data().clone(),
                    log_with_metadata,
                )
                .await?
                {
                    tracing::warn!(
                        tx_hash = %tx_hash,
                        "Found multiple orderbook events in transaction, returning first match"
                    );
                    return Ok(Some(trade));
                }
            }

            if let Ok(take_order_event) = log.log_decode::<TakeOrderV2>() {
                let log_with_metadata = Log {
                    inner: log.inner.clone(),
                    block_hash: receipt.block_hash,
                    block_number: receipt.block_number,
                    block_timestamp: None,
                    transaction_hash: Some(tx_hash),
                    transaction_index: receipt.transaction_index,
                    log_index: Some(log_index as u64),
                    removed: false,
                };

                if let Some(trade) = Self::try_from_take_order_if_target_order(
                    cache,
                    &provider,
                    take_order_event.data().clone(),
                    log_with_metadata,
                    env.order_hash,
                )
                .await?
                {
                    tracing::warn!(
                        tx_hash = %tx_hash,
                        "Found multiple orderbook events in transaction, returning first match"
                    );
                    return Ok(Some(trade));
                }
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use serde_json::json;
    use url::Url;

    fn get_test_env(orderbook: Address, order_hash: B256) -> EvmEnv {
        EvmEnv {
            ws_rpc_url: Url::parse("ws://localhost").unwrap(),
            orderbook,
            order_hash,
        }
    }

    #[tokio::test]
    async fn test_try_from_tx_hash_transaction_not_found() {
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let order_hash =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111");
        let env = get_test_env(orderbook, order_hash);

        let asserter = Asserter::new();

        // Mock transaction receipt response with null (transaction not found)
        asserter.push_success(&json!(null));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err = PartialArbTrade::try_from_tx_hash(tx_hash, provider, &cache, &env)
            .await
            .unwrap_err();

        assert!(matches!(err, TradeConversionError::TransactionNotFound(hash) if hash == tx_hash));
    }

    #[tokio::test]
    async fn test_try_from_tx_hash_no_relevant_events() {
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let order_hash =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111");
        let env = get_test_env(orderbook, order_hash);

        // Create a mock transaction receipt JSON response with logs from different contract
        let receipt_json = json!({
            "transactionHash": tx_hash,
            "transactionIndex": "0x0",
            "blockHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "blockNumber": "0x64",
            "from": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "to": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "contractAddress": null,
            "gasUsed": "0x5208",
            "cumulativeGasUsed": "0xf4240",
            "effectiveGasPrice": "0x3b9aca00",
            "status": "0x1",
            "type": "0x2",
            "logsBloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "logs": [{
                "address": "0x1234567890123456789012345678901234567890", // Different contract
                "topics": [],
                "data": "0x",
                "blockNumber": "0x64",
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "removed": false
            }]
        });

        let asserter = Asserter::new();

        // Mock transaction receipt response
        asserter.push_success(&receipt_json);

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = PartialArbTrade::try_from_tx_hash(tx_hash, provider, &cache, &env)
            .await
            .unwrap();

        assert!(result.is_none());
    }
}
