use alloy::primitives::keccak256;
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::{SolEvent, SolValue};

use super::{EvmEnv, OrderFill, Trade, TradeConversionError};
use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange, ClearV2};
use crate::symbol_cache::SymbolCache;

impl Trade {
    pub(crate) async fn try_from_clear_v2<P: Provider>(
        env: &EvmEnv,
        cache: &SymbolCache,
        provider: P,
        event: ClearV2,
        log: Log,
    ) -> Result<Option<Self>, TradeConversionError> {
        let ClearV2 {
            sender: _,
            alice: alice_order,
            bob: bob_order,
            clearConfig: clear_config,
        } = event;

        let ClearConfig {
            aliceInputIOIndex,
            aliceOutputIOIndex,
            bobInputIOIndex,
            bobOutputIOIndex,
            ..
        } = clear_config;

        let alice_hash_matches = keccak256(alice_order.abi_encode()) == env.order_hash;
        let bob_hash_matches = keccak256(bob_order.abi_encode()) == env.order_hash;

        if !(alice_hash_matches || bob_hash_matches) {
            return Ok(None);
        }

        // we need to get the corresponding AfterClear event as ClearV2 doesn't
        // contain the amounts. so we query the same block number, filter out
        // logs with index lower than the ClearV2 log index and with tx hashes
        // that don't match the ClearV2 tx hash.
        let block_number = log
            .block_number
            .ok_or(TradeConversionError::NoBlockNumber)?;

        let filter = Filter::new()
            .select(block_number)
            .address(env.orderbook)
            .event_signature(AfterClear::SIGNATURE_HASH);

        let after_clear_logs = provider.get_logs(&filter).await?;
        let after_clear_log = after_clear_logs
            .iter()
            .find(|after_clear_log| {
                after_clear_log.transaction_hash == log.transaction_hash
                    && after_clear_log.log_index > log.log_index
            })
            .ok_or(TradeConversionError::NoAfterClearLog)?;

        let after_clear = after_clear_log.log_decode::<AfterClear>()?;

        let ClearStateChange {
            aliceOutput,
            bobOutput,
            aliceInput,
            bobInput,
        } = after_clear.data().clearStateChange;

        if alice_hash_matches {
            let input_index = usize::try_from(aliceInputIOIndex)?;
            let output_index = usize::try_from(aliceOutputIOIndex)?;

            let fill = OrderFill {
                input_index,
                input_amount: aliceInput,
                output_index,
                output_amount: aliceOutput,
            };

            Self::try_from_order_and_fill_details(cache, provider, alice_order, fill, log).await
        } else {
            let input_index = usize::try_from(bobInputIOIndex)?;
            let output_index = usize::try_from(bobOutputIOIndex)?;

            let fill = OrderFill {
                input_index,
                input_amount: bobInput,
                output_index,
                output_amount: bobOutput,
            };

            Self::try_from_order_and_fill_details(cache, provider, bob_order, fill, log).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{IntoLogData as _, U256, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::SolCall;
    use serde_json::json;
    use std::str::FromStr;
    use url::Url;

    use crate::bindings::IERC20::symbolCall;
    use crate::test_utils::get_test_order;
    use crate::trade::SchwabInstruction;

    fn get_env(
        orderbook: alloy::primitives::Address,
        order_hash: alloy::primitives::B256,
    ) -> EvmEnv {
        EvmEnv {
            ws_rpc_url: Url::parse("ws://localhost").unwrap(),
            orderbook,
            order_hash,
        }
    }

    fn get_clear_log(block_number: u64, tx_hash: alloy::primitives::B256) -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: address!("0xfefefefefefefefefefefefefefefefefefefefe"),
                data: alloy::primitives::LogData::empty(),
            },
            block_hash: None,
            block_number: Some(block_number),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        }
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_ok_alice_match() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, order_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let bob_order = order.clone();

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: bob_order,
            clearConfig: clear_config.clone(),
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let clear_log = get_clear_log(1, tx_hash);

        let asserter = Asserter::new();

        // 1. eth_getLogs should return the AfterClear log.
        let after_clear_event = AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::from_str("9000000000000000000").unwrap(), // 9 shares (18 dps)
                bobOutput: U256::ZERO,
                aliceInput: U256::from(100_000_000u64),
                bobInput: U256::ZERO,
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        asserter.push_success(&json!([after_clear_log]));

        // 2+3. Subsequent eth_call symbol fetches.
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trade = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(trade.schwab_ticker, "FOO");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Buy);
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_err_no_after_clear_log() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, order_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let clear_log = get_clear_log(1, tx_hash);

        let asserter = Asserter::new();
        asserter.push_success(&json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap_err();

        assert!(
            matches!(err, TradeConversionError::NoAfterClearLog),
            "got an unexpected error: {err:?}",
        );
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_not_target_order() {
        // Scenario where neither Alice nor Bob order hash matches the target, expect None.
        let order = get_test_order();
        let unrelated_hash =
            keccak256(address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, unrelated_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let clear_log = get_clear_log(1, tx_hash);

        let asserter = Asserter::new();
        asserter.push_success(&json!([]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let res = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_err_no_block_number() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, order_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let mut clear_log = get_clear_log(
            1,
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        );
        clear_log.block_number = None;

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap_err();

        assert!(matches!(err, TradeConversionError::NoBlockNumber));
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_ok_bob_match() {
        let bob_order = get_test_order();
        let bob_order_hash = keccak256(bob_order.abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, bob_order_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::from(0),
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        // Create a different alice order so only bob matches
        let mut alice_order = get_test_order();
        alice_order.nonce =
            fixed_bytes!("0x1111111111111111111111111111111111111111111111111111111111111111");

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: alice_order,
            bob: bob_order,
            clearConfig: clear_config,
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let clear_log = get_clear_log(1, tx_hash);

        let asserter = Asserter::new();

        let after_clear_event = AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::ZERO,
                bobOutput: U256::from_str("9000000000000000000").unwrap(),
                aliceInput: U256::ZERO,
                bobInput: U256::from(100_000_000u64),
            },
        };

        let after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"BARs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let trade = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(trade.schwab_ticker, "BAR");
        assert_eq!(trade.schwab_instruction, SchwabInstruction::Sell);
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_err_invalid_index_conversion() {
        let order = get_test_order();
        let order_hash = keccak256(order.abi_encode());
        let orderbook = address!("0xfefefefefefefefefefefefefefefefefefefefe");
        let env = get_env(orderbook, order_hash);

        let clear_config = ClearConfig {
            aliceInputIOIndex: U256::MAX,
            aliceOutputIOIndex: U256::from(1),
            bobInputIOIndex: U256::from(1),
            bobOutputIOIndex: U256::from(0),
            aliceBountyVaultId: U256::ZERO,
            bobBountyVaultId: U256::ZERO,
        };

        let clear_event = ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: order.clone(),
            bob: order.clone(),
            clearConfig: clear_config,
        };

        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
        let clear_log = get_clear_log(1, tx_hash);

        let asserter = Asserter::new();

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
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        asserter.push_success(&json!([after_clear_log]));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let err = Trade::try_from_clear_v2(&env, &cache, &provider, clear_event, clear_log)
            .await
            .unwrap_err();

        assert!(matches!(err, TradeConversionError::InvalidIndex(_)));
    }
}
