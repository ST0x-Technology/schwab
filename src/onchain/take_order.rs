use alloy::primitives::{B256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolValue;

use crate::bindings::IOrderBookV4::{TakeOrderConfigV3, TakeOrderV2};
use crate::error::OnChainError;
use crate::onchain::trade::{OnchainTrade, OrderFill};
use crate::symbol_cache::SymbolCache;

impl OnchainTrade {
    /// Creates OnchainTrade directly from TakeOrderV2 blockchain events
    pub async fn try_from_take_order_if_target_order<P: Provider>(
        cache: &SymbolCache,
        provider: P,
        event: TakeOrderV2,
        log: Log,
        target_order_hash: B256,
    ) -> Result<Option<Self>, OnChainError> {
        let event_order_hash = keccak256(event.config.order.abi_encode());
        if event_order_hash != target_order_hash {
            return Ok(None);
        }

        let TakeOrderConfigV3 {
            order,
            inputIOIndex,
            outputIOIndex,
            signedContext: _,
        } = event.config;

        let input_index = usize::try_from(inputIOIndex)?;
        let output_index = usize::try_from(outputIOIndex)?;

        let fill = OrderFill {
            input_index,
            input_amount: event.input,
            output_index,
            output_amount: event.output,
        };

        Self::try_from_order_and_fill_details(cache, provider, order, fill, log).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::{SignedContextV1, TakeOrderConfigV3, TakeOrderV2};
    use crate::symbol_cache::SymbolCache;
    use crate::test_utils::{get_test_log, get_test_order};
    use alloy::primitives::{U256, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::{SolCall, SolValue};
    use std::str::FromStr;

    fn create_take_order_event_with_order(
        order: crate::bindings::IOrderBookV4::OrderV3,
    ) -> TakeOrderV2 {
        TakeOrderV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            config: TakeOrderConfigV3 {
                order,
                inputIOIndex: U256::from(0),
                outputIOIndex: U256::from(1),
                signedContext: vec![SignedContextV1 {
                    signer: address!("0x0000000000000000000000000000000000000000"),
                    signature: vec![].into(),
                    context: vec![],
                }],
            },
            input: U256::from(100_000_000u64), // 100 USDC (6 decimals)
            output: U256::from_str("9000000000000000000").unwrap(), // 9 shares (18 decimals)
        }
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_match() {
        let cache = SymbolCache::default();
        let order = get_test_order();
        let target_order_hash = keccak256(order.abi_encode());

        let take_event = create_take_order_event_with_order(order);
        let log = get_test_log();

        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            target_order_hash,
        )
        .await
        .unwrap();

        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPLs1");
        assert!((trade.amount - 9.0).abs() < f64::EPSILON);
        assert_eq!(
            trade.tx_hash,
            fixed_bytes!("0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        );
        assert_eq!(trade.log_index, 293);
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_no_match() {
        let cache = SymbolCache::default();
        let order = get_test_order();

        // Create a different target hash that won't match
        let different_target_hash =
            fixed_bytes!("0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");

        let take_event = create_take_order_event_with_order(order);
        let log = get_test_log();

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            different_target_hash,
        )
        .await
        .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_different_input_output_indices() {
        let cache = SymbolCache::default();
        let order = get_test_order();
        let target_order_hash = keccak256(order.abi_encode());

        let take_event = TakeOrderV2 {
            sender: address!("0x1111111111111111111111111111111111111111"),
            config: TakeOrderConfigV3 {
                order,
                inputIOIndex: U256::from(1), // Different indices
                outputIOIndex: U256::from(0),
                signedContext: vec![SignedContextV1 {
                    signer: address!("0x0000000000000000000000000000000000000000"),
                    signature: vec![].into(),
                    context: vec![],
                }],
            },
            input: U256::from_str("5000000000000000000").unwrap(), // 5 shares (18 decimals)
            output: U256::from(50_000_000u64),                     // 50 USDC (6 decimals)
        };

        let log = get_test_log();

        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            target_order_hash,
        )
        .await
        .unwrap();

        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPLs1");
        assert!((trade.amount - 5.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_with_different_amounts() {
        let cache = SymbolCache::default();
        let order = get_test_order();
        let target_order_hash = keccak256(order.abi_encode());

        let take_event = TakeOrderV2 {
            sender: address!("0x2222222222222222222222222222222222222222"),
            config: TakeOrderConfigV3 {
                order,
                inputIOIndex: U256::from(0),
                outputIOIndex: U256::from(1),
                signedContext: vec![SignedContextV1 {
                    signer: address!("0x0000000000000000000000000000000000000000"),
                    signature: vec![].into(),
                    context: vec![],
                }],
            },
            input: U256::from(200_000_000u64), // 200 USDC
            output: U256::from_str("15000000000000000000").unwrap(), // 15 shares
        };

        let log = get_test_log();

        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            target_order_hash,
        )
        .await
        .unwrap();

        let trade = result.unwrap();
        assert_eq!(trade.symbol, "AAPLs1");
        assert!((trade.amount - 15.0).abs() < f64::EPSILON);
        // Price should be 200 USDC / 15 shares = 13.333... USDC per share
        assert!((trade.price_usdc - 13.333_333_333_333_334).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_zero_amounts() {
        let cache = SymbolCache::default();
        let order = get_test_order();
        let target_order_hash = keccak256(order.abi_encode());

        let take_event = TakeOrderV2 {
            sender: address!("0x3333333333333333333333333333333333333333"),
            config: TakeOrderConfigV3 {
                order,
                inputIOIndex: U256::from(0),
                outputIOIndex: U256::from(1),
                signedContext: vec![SignedContextV1 {
                    signer: address!("0x0000000000000000000000000000000000000000"),
                    signature: vec![].into(),
                    context: vec![],
                }],
            },
            input: U256::ZERO,  // Zero input
            output: U256::ZERO, // Zero output
        };

        let log = get_test_log();

        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"AAPLs1".to_string(),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            target_order_hash,
        )
        .await;

        // Zero amounts should deterministically return Ok(None)
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_invalid_io_index() {
        let cache = SymbolCache::default();
        let order = get_test_order();
        let target_order_hash = keccak256(order.abi_encode());

        let take_event = TakeOrderV2 {
            sender: address!("0x4444444444444444444444444444444444444444"),
            config: TakeOrderConfigV3 {
                order,
                inputIOIndex: U256::from(99), // Invalid index (order only has 2 IOs)
                outputIOIndex: U256::from(1),
                signedContext: vec![SignedContextV1 {
                    signer: address!("0x0000000000000000000000000000000000000000"),
                    signature: vec![].into(),
                    context: vec![],
                }],
            },
            input: U256::from(100_000_000u64),
            output: U256::from_str("9000000000000000000").unwrap(),
        };

        let log = get_test_log();

        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = OnchainTrade::try_from_take_order_if_target_order(
            &cache,
            provider,
            take_event,
            log,
            target_order_hash,
        )
        .await;

        // Should return an error due to invalid IO index
        assert!(result.is_err());
    }
}
