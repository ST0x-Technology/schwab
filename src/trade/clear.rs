use alloy::primitives::keccak256;
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::{SolEvent, SolValue};

use super::{OrderFill, Trade, TradeConversionError};
use crate::Env;
use crate::bindings::IOrderBookV4::{AfterClear, ClearConfig, ClearStateChange, ClearV2};
use crate::symbol_cache::SymbolCache;

impl Trade {
    pub(crate) async fn try_from_clear_v2<P: Provider>(
        env: &Env,
        cache: &SymbolCache,
        provider: P,
        event: ClearV2,
        log: Log,
    ) -> Result<Option<Self>, TradeConversionError> {
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

        if keccak256(alice_order.abi_encode()) == env.order_hash {
            let input_index = usize::try_from(aliceInputIOIndex)?;
            let output_index = usize::try_from(aliceOutputIOIndex)?;

            let fill = OrderFill {
                input_index,
                input_amount: aliceInput,
                output_index,
                output_amount: aliceOutput,
            };

            let trade =
                Trade::try_from_order_and_fill_details(cache, provider, alice_order, fill, log)
                    .await?;

            Ok(Some(trade))
        } else if keccak256(bob_order.abi_encode()) == env.order_hash {
            let input_index = usize::try_from(bobInputIOIndex)?;
            let output_index = usize::try_from(bobOutputIOIndex)?;

            let fill = OrderFill {
                input_index,
                input_amount: bobInput,
                output_index,
                output_amount: bobOutput,
            };

            let trade =
                Trade::try_from_order_and_fill_details(cache, provider, bob_order, fill, log)
                    .await?;

            Ok(Some(trade))
        } else {
            Ok(None)
        }
    }
}
