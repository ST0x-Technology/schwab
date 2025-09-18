use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use tracing::{debug, info};

use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange, ClearV2};
use crate::error::{OnChainError, TradeValidationError};
use crate::onchain::{
    EvmEnv,
    trade::{OnchainTrade, OrderFill},
};
use crate::symbol::cache::SymbolCache;

impl OnchainTrade {
    /// Creates OnchainTrade directly from ClearV2 blockchain events
    pub async fn try_from_clear_v2<P: Provider>(
        env: &EvmEnv,
        cache: &SymbolCache,
        provider: P,
        event: ClearV2,
        log: Log,
    ) -> Result<Option<Self>, OnChainError> {
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

        let alice_owner = alice_order.owner;
        let bob_owner = bob_order.owner;
        let alice_owner_matches = alice_owner == env.order_owner;
        let bob_owner_matches = bob_owner == env.order_owner;

        debug!(
            "ClearV2 owner comparison: alice.owner={:?}, bob.owner={:?}, env.order_owner={:?}, alice_matches={}, bob_matches={}",
            alice_owner, bob_owner, env.order_owner, alice_owner_matches, bob_owner_matches
        );

        if !(alice_owner_matches || bob_owner_matches) {
            info!(
                "ClearV2 event filtered (no owner match): tx_hash={:?}, log_index={}, alice.owner={:?}, bob.owner={:?}, target={:?}",
                log.transaction_hash,
                log.log_index.unwrap_or(0),
                alice_owner,
                bob_owner,
                env.order_owner
            );
            return Ok(None);
        }

        // We need to get the corresponding AfterClear event as ClearV2 doesn't
        // contain the amounts. So we query the same block number, filter out
        // logs with index lower than the ClearV2 log index and with tx hashes
        // that don't match the ClearV2 tx hash.
        let block_number = log
            .block_number
            .ok_or(TradeValidationError::NoBlockNumber)?;

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
            .ok_or(TradeValidationError::NoAfterClearLog)?;

        let after_clear = after_clear_log.log_decode::<AfterClear>()?;

        let ClearStateChange {
            aliceOutput,
            bobOutput,
            aliceInput,
            bobInput,
        } = after_clear.data().clearStateChange;

        let result = if alice_owner_matches {
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
        };

        if let Ok(Some(ref trade)) = result {
            info!(
                "ClearV2 trade created successfully: tx_hash={:?}, log_index={}, symbol={}, amount={}, owner={:?}",
                trade.tx_hash,
                trade.log_index,
                trade.symbol,
                trade.amount,
                if alice_owner_matches {
                    alice_owner
                } else {
                    bob_owner
                }
            );
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange};
    use crate::symbol::cache::SymbolCache;
    use crate::test_utils::{get_test_log, get_test_order};
    use alloy::primitives::{IntoLogData, U256, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::rpc::types::Log;
    use alloy::sol_types::SolCall;
    use serde_json::json;
    use std::str::FromStr;

    fn create_test_env() -> EvmEnv {
        EvmEnv {
            ws_rpc_url: url::Url::parse("ws://localhost:8545").unwrap(),
            orderbook: address!("0x1111111111111111111111111111111111111111"),
            order_owner: get_test_order().owner,
            deployment_block: 1,
        }
    }

    fn create_clear_event(
        alice_order: crate::bindings::IOrderBookV4::OrderV3,
        bob_order: crate::bindings::IOrderBookV4::OrderV3,
    ) -> ClearV2 {
        ClearV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            alice: alice_order,
            bob: bob_order,
            clearConfig: ClearConfig {
                aliceInputIOIndex: U256::from(0),
                aliceOutputIOIndex: U256::from(1),
                bobInputIOIndex: U256::from(1),
                bobOutputIOIndex: U256::from(0),
                aliceBountyVaultId: U256::ZERO,
                bobBountyVaultId: U256::ZERO,
            },
        }
    }

    fn create_after_clear_event() -> AfterClear {
        AfterClear {
            sender: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            clearStateChange: ClearStateChange {
                aliceOutput: U256::from_str("9000000000000000000").unwrap(), // 9 shares (18 dps)
                bobOutput: U256::from(100_000_000u64),                       // 100 USDC (6 dps)
                aliceInput: U256::from(100_000_000u64),                      // 100 USDC (6 dps)
                bobInput: U256::from_str("9000000000000000000").unwrap(),    // 9 shares (18 dps)
            },
        }
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_alice_order_match() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.nonce =
                fixed_bytes!("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(order.clone(), different_order);
        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = create_after_clear_event();
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
            log_index: Some(2), // Higher than clear log
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPL0x".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log)
                .await
                .unwrap();

        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPL0x");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_bob_order_match() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.owner = address!("0xffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(different_order, order.clone());
        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = create_after_clear_event();
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
            log_index: Some(2), // Higher than clear log
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPL0x".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log)
                .await
                .unwrap();

        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPL0x");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_no_order_match() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let different_order1 = {
            let mut order = get_test_order();
            order.owner = address!("0xffffffffffffffffffffffffffffffffffffffff");
            order
        };
        let different_order2 = {
            let mut order = get_test_order();
            order.owner = address!("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
            order
        };

        let clear_event = create_clear_event(different_order1, different_order2);
        let clear_log = get_test_log();

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log)
                .await
                .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_missing_block_number() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.nonce =
                fixed_bytes!("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(order.clone(), different_order);
        let mut clear_log = get_test_log();
        clear_log.block_number = None; // Missing block number

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log).await;

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(crate::error::TradeValidationError::NoBlockNumber)
        ));
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_missing_after_clear_log() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.nonce =
                fixed_bytes!("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(order.clone(), different_order);
        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: address!("0x1111111111111111111111111111111111111111"),
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&json!([])); // No after clear logs found
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log).await;

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(crate::error::TradeValidationError::NoAfterClearLog)
        ));
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_after_clear_wrong_transaction() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.nonce =
                fixed_bytes!("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(order.clone(), different_order);
        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = create_after_clear_event();
        let wrong_after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            )), // Different tx hash
            transaction_index: None,
            log_index: Some(2),
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&json!([wrong_after_clear_log])); // Wrong transaction hash
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log).await;

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(crate::error::TradeValidationError::NoAfterClearLog)
        ));
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_after_clear_wrong_log_index() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();
        let different_order = {
            let mut order = get_test_order();
            order.nonce =
                fixed_bytes!("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
            order
        };

        let clear_event = create_clear_event(order.clone(), different_order);
        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(5), // Higher log index
            removed: false,
        };

        let after_clear_event = create_after_clear_event();
        let wrong_after_clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: after_clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(2), // Lower than clear log index
            removed: false,
        };

        let asserter = Asserter::new();
        asserter.push_success(&json!([wrong_after_clear_log])); // Wrong log index ordering
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log).await;

        assert!(matches!(
            result.unwrap_err(),
            OnChainError::Validation(crate::error::TradeValidationError::NoAfterClearLog)
        ));
    }

    #[tokio::test]
    async fn test_try_from_clear_v2_alice_and_bob_both_match() {
        let env = create_test_env();
        let cache = SymbolCache::default();

        let order = get_test_order();

        // Both Alice and Bob have the target order hash
        let clear_event = create_clear_event(order.clone(), order.clone());
        let orderbook = address!("0x1111111111111111111111111111111111111111");
        let tx_hash =
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

        let clear_log = Log {
            inner: alloy::primitives::Log {
                address: orderbook,
                data: clear_event.to_log_data(),
            },
            block_hash: None,
            block_number: Some(1),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        };

        let after_clear_event = create_after_clear_event();
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

        let asserter = Asserter::new();
        asserter.push_success(&json!([after_clear_log]));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPL0x".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result =
            OnchainTrade::try_from_clear_v2(&env, &cache, provider, clear_event, clear_log)
                .await
                .unwrap();

        // Should process Alice first (alice_hash_matches is checked first)
        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPL0x");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(trade.tx_hash, tx_hash);
        assert_eq!(trade.log_index, 1);
    }
}
