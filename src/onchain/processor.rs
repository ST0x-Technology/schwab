use alloy::primitives::B256;
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;

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

        let trades: Vec<_> = receipt
            .inner
            .logs()
            .iter()
            .enumerate()
            .filter(|(_, log)| {
                (log.topic0() == Some(&ClearV2::SIGNATURE_HASH)
                    || log.topic0() == Some(&TakeOrderV2::SIGNATURE_HASH))
                    && log.address() == env.orderbook
            })
            .collect();

        if trades.len() > 1 {
            tracing::warn!(
                "Found {} potential trades in the tx with hash {tx_hash}, returning first match",
                trades.len()
            );
        }

        for (log_index, log) in trades {
            if let Some(trade) =
                try_convert_log_to_trade(log, log_index, &provider, cache, env).await?
            {
                return Ok(Some(trade));
            }
        }

        Ok(None)
    }
}

async fn try_convert_log_to_trade<P: Provider>(
    log: &Log,
    log_index: usize,
    provider: P,
    cache: &SymbolCache,
    env: &EvmEnv,
) -> Result<Option<PartialArbTrade>, TradeConversionError> {
    let log_with_metadata = Log {
        inner: log.inner.clone(),
        block_hash: log.block_hash,
        block_number: log.block_number,
        block_timestamp: log.block_timestamp,
        transaction_hash: log.transaction_hash,
        transaction_index: log.transaction_index,
        log_index: Some(log_index as u64),
        removed: false,
    };

    if let Ok(clear_event) = log.log_decode::<ClearV2>() {
        return PartialArbTrade::try_from_clear_v2(
            env,
            cache,
            &provider,
            clear_event.data().clone(),
            log_with_metadata,
        )
        .await;
    }

    if let Ok(take_order_event) = log.log_decode::<TakeOrderV2>() {
        return PartialArbTrade::try_from_take_order_if_target_order(
            cache,
            &provider,
            take_order_event.data().clone(),
            log_with_metadata,
            env.order_hash,
        )
        .await;
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::hex;
    use alloy::primitives::{Address, IntoLogData, U256, address, fixed_bytes, keccak256};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::{SolCall, SolEvent, SolValue};
    use serde_json::json;
    use std::str::FromStr;
    use url::Url;

    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange};
    use crate::schwab::SchwabInstruction;
    use crate::test_utils::get_test_order;

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
                "address": "0x1234567890123456789012345678901234567890",
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
        asserter.push_success(&receipt_json);

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = PartialArbTrade::try_from_tx_hash(tx_hash, provider, &cache, &env)
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_try_from_tx_hash_with_clear_v2_success() {
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let env = get_test_env(orderbook, order_hash);

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: U256::from(0),
                aliceOutputIOIndex: U256::from(1),
                bobInputIOIndex: U256::from(1),
                bobOutputIOIndex: U256::from(0),
                aliceBountyVaultId: U256::ZERO,
                bobBountyVaultId: U256::ZERO,
            },
        };

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
                "address": orderbook,
                "topics": [ClearV2::SIGNATURE_HASH],
                "data": format!("0x{}", hex::encode(clear_event.into_log_data().data)),
                "blockNumber": "0x64",
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "removed": false
            }]
        });

        let asserter = Asserter::new();
        asserter.push_success(&receipt_json);

        let after_clear_event = AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::from_str("9000000000000000000").unwrap(),
                bobOutput: U256::ZERO,
                aliceInput: U256::from(100_000_000u64),
                bobInput: U256::ZERO,
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.into_log_data(),
            },
            block_hash: Some(fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            )),
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: Some(0),
            log_index: Some(1),
            removed: false,
        };

        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trade = PartialArbTrade::try_from_tx_hash(tx_hash, provider, &cache, &env)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.schwab_ticker, "FOO");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
    }

    #[tokio::test]
    async fn test_try_from_tx_hash_orderbook_event_not_target_order() {
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let order = get_test_order();
        let different_order_hash =
            fixed_bytes!("0x9999999999999999999999999999999999999999999999999999999999999999");
        let env = get_test_env(orderbook, different_order_hash);

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: ClearConfig {
                aliceInputIOIndex: U256::from(0),
                aliceOutputIOIndex: U256::from(1),
                bobInputIOIndex: U256::from(1),
                bobOutputIOIndex: U256::from(0),
                aliceBountyVaultId: U256::ZERO,
                bobBountyVaultId: U256::ZERO,
            },
        };

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
                "address": orderbook,
                "topics": [ClearV2::SIGNATURE_HASH],
                "data": format!("0x{}", hex::encode(clear_event.into_log_data().data)),
                "blockNumber": "0x64",
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": "0x0",
                "removed": false
            }]
        });

        let asserter = Asserter::new();
        asserter.push_success(&receipt_json);

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let result = PartialArbTrade::try_from_tx_hash(tx_hash, provider, &cache, &env)
            .await
            .unwrap();

        assert!(result.is_none());
    }
}
