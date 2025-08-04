use alloy::primitives::{B256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolValue;

use super::{OrderFill, PartialArbTrade, TradeConversionError};
use crate::bindings::IOrderBookV4::{TakeOrderConfigV3, TakeOrderV2};
use crate::symbol_cache::SymbolCache;

impl PartialArbTrade {
    pub(crate) async fn try_from_take_order_if_target_order<P: Provider>(
        cache: &SymbolCache,
        provider: P,
        event: TakeOrderV2,
        log: Log,
        target_order_hash: B256,
    ) -> Result<Option<Self>, TradeConversionError> {
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
    use alloy::primitives::{U256, address, fixed_bytes};
    use alloy::providers::{ProviderBuilder, mock::Asserter};
    use alloy::sol_types::SolCall;
    use std::str::FromStr;

    use super::*;
    use crate::bindings::IERC20::symbolCall;
    use crate::bindings::IOrderBookV4::OrderV3;
    use crate::bindings::IOrderBookV4::TakeOrderConfigV3;
    use crate::test_utils::get_test_order;

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_match() {
        // mock symbol fetches
        let asserter = Asserter::new();
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"USDC".to_string(),
        ));
        asserter.push_success(&<symbolCall as SolCall>::abi_encode_returns(
            &"FOOs1".to_string(),
        ));

        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let order = get_test_order();
        let event = get_event(order.clone(), 0, 1);
        let log = get_log();

        let order_hash = keccak256(order.abi_encode());

        let trade = PartialArbTrade::try_from_take_order_if_target_order(
            &cache, &provider, event, log, order_hash,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(trade.schwab_ticker, "FOO");
        assert_eq!(
            trade.schwab_instruction,
            super::super::SchwabInstruction::Buy
        );
    }

    #[tokio::test]
    async fn test_try_from_take_order_if_target_order_mismatch() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let cache = SymbolCache::default();

        let order = get_test_order();
        let event = get_event(order.clone(), 0, 1);
        let log = get_log();

        let unrelated_hash = B256::ZERO;

        let res = PartialArbTrade::try_from_take_order_if_target_order(
            &cache,
            &provider,
            event,
            log,
            unrelated_hash,
        )
        .await
        .unwrap();

        assert!(res.is_none());
    }

    fn get_event(order: OrderV3, input_index: usize, output_index: usize) -> TakeOrderV2 {
        let config = TakeOrderConfigV3 {
            order,
            inputIOIndex: U256::from(input_index),
            outputIOIndex: U256::from(output_index),
            signedContext: vec![],
        };

        TakeOrderV2 {
            sender: address!("0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            config,
            input: U256::from(100_000_000u64),
            output: U256::from_str("9000000000000000000").unwrap(), // 9 shares with 18 decimals
        }
    }

    fn get_log() -> Log {
        Log {
            inner: alloy::primitives::Log {
                address: address!("0xfefefefefefefefefefefefefefefefefefefefe"),
                data: alloy::primitives::LogData::empty(),
            },
            block_hash: None,
            block_number: Some(12345),
            block_timestamp: None,
            transaction_hash: Some(fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            )),
            transaction_index: None,
            log_index: Some(1),
            removed: false,
        }
    }
}
