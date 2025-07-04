use alloy::primitives::{B256, keccak256};
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolValue;

use super::{OrderFill, Trade, TradeConversionError};
use crate::bindings::IOrderBookV4::{TakeOrderConfigV3, TakeOrderV2};
use crate::symbol_cache::SymbolCache;

impl Trade {
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

        let trade =
            Trade::try_from_order_and_fill_details(cache, provider, order, fill, log).await?;

        Ok(Some(trade))
    }
}
