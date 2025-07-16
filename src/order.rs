use serde::Serialize;

use crate::trade::SchwabInstruction;

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Instrument {
    pub symbol: String,
    pub asset_type: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrderLeg {
    pub order_leg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leg_id: Option<u32>,
    pub instrument: Instrument,
    pub instruction: SchwabInstruction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_effect: Option<String>,
    pub quantity: u32,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OrderRequestMinimal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    pub session: String,
    pub duration: String,
    pub order_type: String,
    pub complex_order_strategy_type: String,
    pub quantity: u32,
    pub tax_lot_method: String,
    pub order_leg_collection: Vec<OrderLeg>,
    pub order_strategy_type: String,
}

impl OrderRequestMinimal {
    pub fn market_equity(symbol: &str, qty: u32, instruction: SchwabInstruction) -> Self {
        Self {
            price: None,
            session: "NORMAL".into(),
            duration: "DAY".into(),
            order_type: "MARKET".into(),
            complex_order_strategy_type: "NONE".into(),
            quantity: qty,
            tax_lot_method: "FIFO".into(),
            order_leg_collection: vec![OrderLeg {
                order_leg_type: "EQUITY".into(),
                leg_id: Some(1),
                instrument: Instrument {
                    symbol: symbol.into(),
                    asset_type: "EQUITY".into(),
                },
                instruction,
                position_effect: Some("OPENING".into()),
                quantity: qty,
            }],
            order_strategy_type: "SINGLE".into(),
        }
    }
}
