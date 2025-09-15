use serde::{Deserialize, Deserializer, Serialize};

use super::price_cents_from_db_i64;
use crate::error::OnChainError;

/// Deserialize orderId from Schwab API as int64 and convert to string for database compatibility.
///
/// NOTE: Schwab API spec defines orderId as int64, but our database schema stores it as TEXT.
/// This conversion bridges the API format to our storage format. We may want to change the
/// database schema to INTEGER before production deployment.
fn deserialize_order_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt_value = Option::<u64>::deserialize(deserializer)?;
    Ok(opt_value.map(|order_id| order_id.to_string()))
}

/// Order status enum matching Schwab API states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum OrderStatus {
    Queued,
    Working,
    Filled,
    Canceled,
    Rejected,
    PendingActivation,
    PendingReview,
    Accepted,
    AwaitingParentOrder,
    AwaitingCondition,
    AwaitingManualReview,
    AwaitingStopCondition,
    Expired,
    New,
    AwaitingReleaseTime,
    PendingReplace,
    Replaced,
}

/// Order status response from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderStatusResponse {
    #[serde(default, deserialize_with = "deserialize_order_id")]
    pub order_id: Option<String>,
    pub status: Option<OrderStatus>,
    pub filled_quantity: Option<f64>,
    pub remaining_quantity: Option<f64>,
    pub entered_time: Option<String>,
    pub close_time: Option<String>,
    #[serde(rename = "orderActivityCollection")]
    pub order_activity_collection: Option<Vec<OrderActivity>>,
    #[serde(rename = "commissionAndFee")]
    pub commission_and_fee: Option<CommissionAndFee>,
}

/// Order activity from Schwab API orderActivityCollection
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderActivity {
    pub activity_type: Option<String>,
    pub execution_legs: Option<Vec<ExecutionLeg>>,
}

/// Execution leg details from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionLeg {
    pub quantity: f64,
    pub price: f64,
}

/// Commission and fee data from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommissionAndFee {
    pub commission: Option<Commission>,
    pub fee: Option<Fees>,
    pub true_commission: Option<f64>,
}

/// Commission details from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Commission {
    pub commission_legs: Option<Vec<CommissionLeg>>,
}

/// Commission leg containing commission values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommissionLeg {
    pub commission_values: Option<Vec<CommissionValue>>,
}

/// Individual commission value with type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommissionValue {
    pub value: f64,
    #[serde(rename = "type")]
    pub fee_type: FeeType,
}

/// Fee details from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Fees {
    pub fee_legs: Option<Vec<FeeLeg>>,
}

/// Fee leg containing fee values
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeLeg {
    pub fee_values: Option<Vec<FeeValue>>,
}

/// Individual fee value with type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FeeValue {
    pub value: f64,
    #[serde(rename = "type")]
    pub fee_type: FeeType,
}

/// Fee type enum with all possible values from Schwab API
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum FeeType {
    Commission,
    SecFee,
    TafFee,
    NsccFee,
    AdditionalFee,
    MiscFee,
    CommissionAdjustment,
    IndexOptionFee,
    ForeignExchangeFee,
    RegulatoryFee,
    OtherCharges,
    LowProceedsFee,
    BaseCharge,
    GSLFee,
    STTFee,
    TransactionFee,
    ServiceCharge,
    SpecialTransactionFee,
    ClearingFee,
    ExchangeFee,
    FloorBrokerageFee,
    CDSLFee,
    StampDuty,
    PassThroughFee,
    ActivityAssessmentFee,
}

impl OrderStatusResponse {
    /// Calculate weighted average fill price from orderActivityCollection
    pub(crate) fn calculate_weighted_average_price(&self) -> Option<f64> {
        let activities = self.order_activity_collection.as_ref()?;

        let (total_value, total_quantity) = activities
            .iter()
            .filter_map(|activity| activity.execution_legs.as_ref())
            .flat_map(|legs| legs.iter())
            .map(|leg| (leg.price * leg.quantity, leg.quantity))
            .fold((0.0, 0.0), |(acc_value, acc_qty), (value, qty)| {
                (acc_value + value, acc_qty + qty)
            });

        if total_quantity > 0.0 {
            Some(total_value / total_quantity)
        } else {
            None
        }
    }

    /// Convert price to cents for database storage
    pub(crate) fn price_in_cents(&self) -> Result<Option<u64>, OnChainError> {
        self.calculate_weighted_average_price().map_or_else(
            || Ok(None),
            |price| {
                // Convert dollars to cents and round
                #[allow(clippy::cast_possible_truncation)]
                let cents = (price * 100.0).round() as i64;
                price_cents_from_db_i64(cents).map(Some)
            },
        )
    }

    /// Check if order is completely filled
    pub(crate) const fn is_filled(&self) -> bool {
        matches!(self.status, Some(OrderStatus::Filled))
    }

    /// Check if order is still pending/working
    #[cfg(test)]
    pub(crate) const fn is_pending(&self) -> bool {
        matches!(
            self.status,
            Some(
                OrderStatus::Queued
                    | OrderStatus::Working
                    | OrderStatus::PendingActivation
                    | OrderStatus::PendingReview
                    | OrderStatus::Accepted
                    | OrderStatus::AwaitingParentOrder
                    | OrderStatus::AwaitingCondition
                    | OrderStatus::AwaitingManualReview
                    | OrderStatus::AwaitingStopCondition
                    | OrderStatus::New
                    | OrderStatus::AwaitingReleaseTime
                    | OrderStatus::PendingReplace
            )
        )
    }

    /// Check if order was canceled or rejected
    pub(crate) const fn is_terminal_failure(&self) -> bool {
        matches!(
            self.status,
            Some(OrderStatus::Canceled | OrderStatus::Rejected | OrderStatus::Expired)
        )
    }

    /// Extract commission amount from commissionAndFee structure
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn extract_commission_cents(&self) -> u64 {
        self.commission_and_fee
            .as_ref()
            .and_then(|cf| cf.commission.as_ref())
            .and_then(|commission| commission.commission_legs.as_ref())
            .map_or(0, |legs| {
                legs.iter()
                    .filter_map(|leg| leg.commission_values.as_ref())
                    .flat_map(|values| values.iter())
                    .filter(|value| matches!(value.fee_type, FeeType::Commission))
                    .map(|value| (value.value * 100.0).round() as u64)
                    .sum()
            })
    }

    /// Extract SEC fee amount from commissionAndFee structure
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn extract_sec_fee_cents(&self) -> u64 {
        self.commission_and_fee
            .as_ref()
            .and_then(|cf| cf.fee.as_ref())
            .and_then(|fees| fees.fee_legs.as_ref())
            .map_or(0, |legs| {
                legs.iter()
                    .filter_map(|leg| leg.fee_values.as_ref())
                    .flat_map(|values| values.iter())
                    .filter(|value| matches!(value.fee_type, FeeType::SecFee))
                    .map(|value| (value.value * 100.0).round() as u64)
                    .sum()
            })
    }

    /// Extract TAF fee amount from commissionAndFee structure
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn extract_taf_fee_cents(&self) -> u64 {
        self.commission_and_fee
            .as_ref()
            .and_then(|cf| cf.fee.as_ref())
            .and_then(|fees| fees.fee_legs.as_ref())
            .map_or(0, |legs| {
                legs.iter()
                    .filter_map(|leg| leg.fee_values.as_ref())
                    .flat_map(|values| values.iter())
                    .filter(|value| matches!(value.fee_type, FeeType::TafFee))
                    .map(|value| (value.value * 100.0).round() as u64)
                    .sum()
            })
    }

    /// Extract other fees (excluding commission, SEC fee, TAF fee) from commissionAndFee structure
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) fn extract_other_fees_cents(&self) -> u64 {
        self.commission_and_fee.as_ref().map_or(0, |cf| {
            let commission_other_fees = cf
                .commission
                .as_ref()
                .and_then(|commission| commission.commission_legs.as_ref())
                .map_or(0, |legs| {
                    legs.iter()
                        .filter_map(|leg| leg.commission_values.as_ref())
                        .flat_map(|values| values.iter())
                        .filter(|value| {
                            !matches!(
                                value.fee_type,
                                FeeType::Commission | FeeType::SecFee | FeeType::TafFee
                            )
                        })
                        .map(|value| (value.value * 100.0).round() as u64)
                        .sum::<u64>()
                });

            let fee_other_fees = cf
                .fee
                .as_ref()
                .and_then(|fees| fees.fee_legs.as_ref())
                .map_or(0, |legs| {
                    legs.iter()
                        .filter_map(|leg| leg.fee_values.as_ref())
                        .flat_map(|values| values.iter())
                        .filter(|value| {
                            !matches!(value.fee_type, FeeType::SecFee | FeeType::TafFee)
                        })
                        .map(|value| (value.value * 100.0).round() as u64)
                        .sum::<u64>()
                });

            commission_other_fees + fee_other_fees
        })
    }

    /// Calculate total fees across all legs and values - ESSENTIAL for P&L calculations
    pub(crate) fn extract_total_fees_cents(&self) -> u64 {
        self.extract_commission_cents()
            + self.extract_sec_fee_cents()
            + self.extract_taf_fee_cents()
            + self.extract_other_fees_cents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_status_serialization() {
        let status = OrderStatus::Filled;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"FILLED\"");

        let deserialized: OrderStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, status);
    }

    #[test]
    fn test_order_status_response_deserialization() {
        // Test parsing a typical API response with orderActivityCollection
        let json_response = r#"{
            "orderId": 1004055538123,
            "status": "FILLED",
            "filledQuantity": 100.0,
            "remainingQuantity": 0.0,
            "enteredTime": "2023-10-15T10:25:00Z",
            "closeTime": "2023-10-15T10:30:00Z",
            "orderActivityCollection": [{
                "activityType": "EXECUTION",
                "executionLegs": [{
                    "executionId": "EXEC123",
                    "quantity": 100.0,
                    "price": 150.25,
                    "time": "2023-10-15T10:30:00Z"
                }]
            }]
        }"#;

        let response: OrderStatusResponse = serde_json::from_str(json_response).unwrap();

        assert_eq!(response.order_id, Some("1004055538123".to_string()));
        assert_eq!(response.status, Some(OrderStatus::Filled));
        assert!((response.filled_quantity.unwrap() - 100.0).abs() < f64::EPSILON);
        assert!(response.remaining_quantity.unwrap().abs() < f64::EPSILON);
        assert_eq!(
            response.order_activity_collection.as_ref().unwrap().len(),
            1
        );
    }

    #[test]
    fn test_calculate_weighted_average_price_single_leg() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![ExecutionLeg {
                    quantity: 100.0,
                    price: 150.25,
                }]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), Some(150.25));
    }

    #[test]
    fn test_calculate_weighted_average_price_multiple_legs() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(200.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:10Z".to_string()),
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![
                    ExecutionLeg {
                        quantity: 100.0,
                        price: 150.00,
                    },
                    ExecutionLeg {
                        quantity: 100.0,
                        price: 151.00,
                    },
                ]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), Some(150.5));
    }

    #[test]
    fn test_calculate_weighted_average_price_weighted() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(300.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:10Z".to_string()),
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![
                    ExecutionLeg {
                        quantity: 200.0,
                        price: 150.00,
                    },
                    ExecutionLeg {
                        quantity: 100.0,
                        price: 153.00,
                    },
                ]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), Some(151.0));
    }

    #[test]
    fn test_calculate_weighted_average_price_empty_legs() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Working),
            filled_quantity: Some(0.0),
            remaining_quantity: Some(100.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
            order_activity_collection: Some(vec![]),
            commission_and_fee: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), None);
    }

    #[test]
    fn test_price_in_cents_conversion() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![ExecutionLeg {
                    quantity: 100.0,
                    price: 150.25,
                }]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.price_in_cents().unwrap(), Some(15025));
    }

    #[test]
    fn test_price_in_cents_no_executions() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Working),
            filled_quantity: Some(0.0),
            remaining_quantity: Some(100.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
            order_activity_collection: Some(vec![]),
            commission_and_fee: None,
        };

        assert_eq!(response.price_in_cents().unwrap(), None);
    }

    #[test]
    fn test_price_in_cents_rounding() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![ExecutionLeg {
                    quantity: 100.0,
                    price: 150.254,
                }]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.price_in_cents().unwrap(), Some(15025));
    }

    #[test]
    fn test_is_filled() {
        let mut response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![]),
            commission_and_fee: None,
        };

        assert!(response.is_filled());

        response.status = Some(OrderStatus::Working);
        assert!(!response.is_filled());
    }

    #[test]
    fn test_is_pending() {
        let pending_states = [
            OrderStatus::Queued,
            OrderStatus::Working,
            OrderStatus::PendingActivation,
            OrderStatus::PendingReview,
            OrderStatus::Accepted,
            OrderStatus::AwaitingParentOrder,
            OrderStatus::AwaitingCondition,
            OrderStatus::AwaitingManualReview,
            OrderStatus::AwaitingStopCondition,
            OrderStatus::New,
            OrderStatus::AwaitingReleaseTime,
            OrderStatus::PendingReplace,
        ];

        for status in pending_states {
            let response = OrderStatusResponse {
                order_id: Some("1004055538123".to_string()),
                status: Some(status),
                filled_quantity: Some(0.0),
                remaining_quantity: Some(100.0),
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: None,
                order_activity_collection: Some(vec![]),
                commission_and_fee: None,
            };
            assert!(response.is_pending(), "Status {status:?} should be pending");
        }

        let non_pending_states = [
            OrderStatus::Filled,
            OrderStatus::Canceled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
            OrderStatus::Replaced,
        ];

        for status in non_pending_states {
            let response = OrderStatusResponse {
                order_id: Some("1004055538123".to_string()),
                status: Some(status),
                filled_quantity: Some(100.0),
                remaining_quantity: Some(0.0),
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: Some("2023-10-15T10:30:00Z".to_string()),
                order_activity_collection: Some(vec![]),
                commission_and_fee: None,
            };
            assert!(
                !response.is_pending(),
                "Status {status:?} should not be pending"
            );
        }
    }

    #[test]
    fn test_is_terminal_failure() {
        let failure_states = [
            OrderStatus::Canceled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ];

        for status in failure_states {
            let response = OrderStatusResponse {
                order_id: Some("1004055538123".to_string()),
                status: Some(status),
                filled_quantity: Some(0.0),
                remaining_quantity: Some(100.0),
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: Some("2023-10-15T10:30:00Z".to_string()),
                order_activity_collection: Some(vec![]),
                commission_and_fee: None,
            };
            assert!(
                response.is_terminal_failure(),
                "Status {status:?} should be terminal failure"
            );
        }

        let non_failure_states = [
            OrderStatus::Filled,
            OrderStatus::Working,
            OrderStatus::Queued,
            OrderStatus::New,
        ];

        for status in non_failure_states {
            let response = OrderStatusResponse {
                order_id: Some("1004055538123".to_string()),
                status: Some(status),
                filled_quantity: Some(0.0),
                remaining_quantity: Some(100.0),
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: None,
                order_activity_collection: Some(vec![]),
                commission_and_fee: None,
            };
            assert!(
                !response.is_terminal_failure(),
                "Status {status:?} should not be terminal failure"
            );
        }
    }

    #[test]
    fn test_complex_api_response_parsing() {
        let json_response = r#"
        {
            "orderId": 1004055538999,
            "status": "FILLED",
            "filledQuantity": 200.0,
            "remainingQuantity": 0.0,
            "enteredTime": "2023-10-15T10:25:00Z",
            "closeTime": "2023-10-15T10:30:05Z",
            "orderActivityCollection": [{
                "activityType": "EXECUTION",
                "executionLegs": [
                    {
                        "executionId": "EXEC001",
                        "quantity": 150.0,
                        "price": 100.25,
                        "time": "2023-10-15T10:30:00Z"
                    },
                    {
                        "executionId": "EXEC002",
                        "quantity": 50.0,
                        "price": 100.75,
                        "time": "2023-10-15T10:30:05Z"
                    }
                ]
            }]
        }
        "#;

        let response: OrderStatusResponse = serde_json::from_str(json_response).unwrap();

        assert_eq!(response.order_id, Some("1004055538999".to_string()));
        assert_eq!(response.status, Some(OrderStatus::Filled));
        assert!((response.filled_quantity.unwrap() - 200.0).abs() < f64::EPSILON);
        assert!(response.remaining_quantity.unwrap().abs() < f64::EPSILON);
        assert_eq!(
            response.order_activity_collection.as_ref().unwrap().len(),
            1
        );

        // Test weighted average: (150 * 100.25 + 50 * 100.75) / 200 = (15037.5 + 5037.5) / 200 = 100.375
        let avg_price = response.calculate_weighted_average_price().unwrap();
        assert!((avg_price - 100.375).abs() < f64::EPSILON);
        assert_eq!(response.price_in_cents().unwrap(), Some(10038)); // Rounded
    }

    #[test]
    fn test_edge_case_zero_quantity_legs() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Working),
            filled_quantity: Some(0.0),
            remaining_quantity: Some(100.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
            order_activity_collection: Some(vec![OrderActivity {
                activity_type: Some("EXECUTION".to_string()),
                execution_legs: Some(vec![ExecutionLeg {
                    quantity: 0.0,
                    price: 150.25,
                }]),
            }]),
            commission_and_fee: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), None);
        assert_eq!(response.price_in_cents().unwrap(), None);
    }

    #[test]
    fn test_actual_schwab_api_response_filled_order() {
        // This is the actual response format returned by Schwab API for a filled GME order
        let actual_response = r#"{
            "session":"NORMAL",
            "duration":"DAY",
            "orderType":"MARKET",
            "complexOrderStrategyType":"NONE",
            "quantity":1.0,
            "filledQuantity":1.0,
            "remainingQuantity":0.0,
            "requestedDestination":"AUTO",
            "destinationLinkName":"HRTF",
            "orderLegCollection":[{
                "orderLegType":"EQUITY",
                "legId":1,
                "instrument":{
                    "assetType":"EQUITY",
                    "cusip":"36467W109",
                    "symbol":"GME",
                    "instrumentId":4430271
                },
                "instruction":"BUY",
                "positionEffect":"OPENING",
                "quantity":1.0
            }],
            "orderStrategyType":"SINGLE",
            "orderId":1004055538153,
            "cancelable":false,
            "editable":false,
            "status":"FILLED",
            "enteredTime":"2025-08-29T17:15:17+0000",
            "closeTime":"2025-08-29T17:15:18+0000",
            "tag":"TA_nickmagliocchetticom1751890824",
            "accountNumber":49359741,
            "orderActivityCollection":[{
                "activityType":"EXECUTION",
                "activityId":102102029816,
                "executionType":"FILL",
                "quantity":1.0,
                "orderRemainingQuantity":0.0,
                "executionLegs":[{
                    "legId":1,
                    "quantity":1.0,
                    "mismarkedQuantity":0.0,
                    "price":22.7299,
                    "time":"2025-08-29T17:15:18+0000",
                    "instrumentId":4430271
                }]
            }]
        }"#;

        // This should parse successfully now
        let parsed: OrderStatusResponse =
            serde_json::from_str(actual_response).expect("Should parse actual Schwab API response");

        // Verify the parsed values
        assert_eq!(parsed.order_id, Some("1004055538153".to_string()));
        assert_eq!(parsed.status, Some(OrderStatus::Filled));
        assert!((parsed.filled_quantity.unwrap() - 1.0).abs() < f64::EPSILON);
        assert!(parsed.remaining_quantity.unwrap().abs() < f64::EPSILON);
        assert_eq!(
            parsed.entered_time,
            Some("2025-08-29T17:15:17+0000".to_string())
        );
        assert_eq!(
            parsed.close_time,
            Some("2025-08-29T17:15:18+0000".to_string())
        );

        // Verify price extraction from orderActivityCollection
        let avg_price = parsed.calculate_weighted_average_price();
        assert!(avg_price.is_some());
        assert!((avg_price.unwrap() - 22.7299).abs() < f64::EPSILON);

        // Verify price in cents conversion
        let price_cents = parsed.price_in_cents().unwrap();
        assert_eq!(price_cents, Some(2273)); // 22.7299 * 100 rounded = 2273 cents
    }

    #[test]
    fn test_order_id_as_number() {
        // Test that we can handle orderId as a number (actual Schwab format)
        let response_json = r#"{
            "orderId": 1004055538153,
            "status": "FILLED",
            "filledQuantity": 1.0,
            "remainingQuantity": 0.0
        }"#;

        let parsed: OrderStatusResponse =
            serde_json::from_str(response_json).expect("Should parse orderId as number");

        assert_eq!(parsed.order_id, Some("1004055538153".to_string()));
        assert_eq!(parsed.status, Some(OrderStatus::Filled));
    }

    #[test]
    fn test_order_id_as_string_should_fail() {
        // Test that we reject orderId as string (not the actual API format)
        let response_json = r#"{
            "orderId": "ORDER123",
            "status": "WORKING",
            "filledQuantity": 0.0,
            "remainingQuantity": 100.0
        }"#;

        let result = serde_json::from_str::<OrderStatusResponse>(response_json);
        assert!(result.is_err(), "Should reject string orderId format");
    }

    #[test]
    fn test_order_id_missing() {
        // Test that we can handle missing orderId field
        let response_json = r#"{
            "status": "QUEUED",
            "filledQuantity": 0.0,
            "remainingQuantity": 100.0
        }"#;

        let parsed: OrderStatusResponse =
            serde_json::from_str(response_json).expect("Should parse response without orderId");

        assert_eq!(parsed.order_id, None);
        assert_eq!(parsed.status, Some(OrderStatus::Queued));
    }

    #[test]
    fn test_missing_optional_fields() {
        let minimal_response = r#"{
            "status": "QUEUED"
        }"#;

        let parsed: OrderStatusResponse =
            serde_json::from_str(minimal_response).expect("Should parse minimal response");

        assert_eq!(parsed.order_id, None);
        assert_eq!(parsed.status, Some(OrderStatus::Queued));
        assert_eq!(parsed.filled_quantity, None);
        assert_eq!(parsed.remaining_quantity, None);
        assert_eq!(parsed.entered_time, None);
        assert_eq!(parsed.close_time, None);
        assert_eq!(parsed.order_activity_collection, None);
    }

    #[test]
    fn test_missing_status_field_none() {
        let no_status_response = r#"{
            "orderId": 1004055538123,
            "filledQuantity": 0.0,
            "remainingQuantity": 100.0
        }"#;

        let parsed: OrderStatusResponse =
            serde_json::from_str(no_status_response).expect("Should parse response without status");

        assert_eq!(parsed.order_id, Some("1004055538123".to_string()));
        assert_eq!(parsed.status, None);
        assert_eq!(parsed.filled_quantity, Some(0.0));
        assert_eq!(parsed.remaining_quantity, Some(100.0));
    }

    #[test]
    fn test_fee_extraction_with_commission_and_fees() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![]),
            commission_and_fee: Some(CommissionAndFee {
                commission: Some(Commission {
                    commission_legs: Some(vec![CommissionLeg {
                        commission_values: Some(vec![CommissionValue {
                            value: 0.65,
                            fee_type: FeeType::Commission,
                        }]),
                    }]),
                }),
                fee: Some(Fees {
                    fee_legs: Some(vec![FeeLeg {
                        fee_values: Some(vec![
                            FeeValue {
                                value: 0.01,
                                fee_type: FeeType::SecFee,
                            },
                            FeeValue {
                                value: 0.02,
                                fee_type: FeeType::TafFee,
                            },
                            FeeValue {
                                value: 0.05,
                                fee_type: FeeType::RegulatoryFee,
                            },
                        ]),
                    }]),
                }),
                true_commission: Some(0.65),
            }),
        };

        assert_eq!(response.extract_commission_cents(), 65);
        assert_eq!(response.extract_sec_fee_cents(), 1);
        assert_eq!(response.extract_taf_fee_cents(), 2);
        assert_eq!(response.extract_other_fees_cents(), 5);
        assert_eq!(response.extract_total_fees_cents(), 73);
    }

    #[test]
    fn test_fee_extraction_no_commission_and_fee() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![]),
            commission_and_fee: None,
        };

        assert_eq!(response.extract_commission_cents(), 0);
        assert_eq!(response.extract_sec_fee_cents(), 0);
        assert_eq!(response.extract_taf_fee_cents(), 0);
        assert_eq!(response.extract_other_fees_cents(), 0);
        assert_eq!(response.extract_total_fees_cents(), 0);
    }

    #[test]
    fn test_fee_extraction_empty_legs() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![]),
            commission_and_fee: Some(CommissionAndFee {
                commission: Some(Commission {
                    commission_legs: Some(vec![]),
                }),
                fee: Some(Fees {
                    fee_legs: Some(vec![]),
                }),
                true_commission: Some(0.0),
            }),
        };

        assert_eq!(response.extract_commission_cents(), 0);
        assert_eq!(response.extract_sec_fee_cents(), 0);
        assert_eq!(response.extract_taf_fee_cents(), 0);
        assert_eq!(response.extract_other_fees_cents(), 0);
        assert_eq!(response.extract_total_fees_cents(), 0);
    }

    #[test]
    fn test_fee_extraction_multiple_legs_and_values() {
        let response = OrderStatusResponse {
            order_id: Some("1004055538123".to_string()),
            status: Some(OrderStatus::Filled),
            filled_quantity: Some(100.0),
            remaining_quantity: Some(0.0),
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
            order_activity_collection: Some(vec![]),
            commission_and_fee: Some(CommissionAndFee {
                commission: Some(Commission {
                    commission_legs: Some(vec![
                        CommissionLeg {
                            commission_values: Some(vec![CommissionValue {
                                value: 0.50,
                                fee_type: FeeType::Commission,
                            }]),
                        },
                        CommissionLeg {
                            commission_values: Some(vec![CommissionValue {
                                value: 0.15,
                                fee_type: FeeType::Commission,
                            }]),
                        },
                    ]),
                }),
                fee: Some(Fees {
                    fee_legs: Some(vec![
                        FeeLeg {
                            fee_values: Some(vec![
                                FeeValue {
                                    value: 0.005,
                                    fee_type: FeeType::SecFee,
                                },
                                FeeValue {
                                    value: 0.01,
                                    fee_type: FeeType::TafFee,
                                },
                            ]),
                        },
                        FeeLeg {
                            fee_values: Some(vec![
                                FeeValue {
                                    value: 0.005,
                                    fee_type: FeeType::SecFee,
                                },
                                FeeValue {
                                    value: 0.02,
                                    fee_type: FeeType::TafFee,
                                },
                            ]),
                        },
                    ]),
                }),
                true_commission: Some(0.65),
            }),
        };

        assert_eq!(response.extract_commission_cents(), 65); // 50 + 15
        assert_eq!(response.extract_sec_fee_cents(), 2); // 0.005*100=0.5→1 + 0.005*100=0.5→1 = 2 cents
        assert_eq!(response.extract_taf_fee_cents(), 3); // 0.01*100=1 + 0.02*100=2 = 3 cents
        assert_eq!(response.extract_other_fees_cents(), 0);
        assert_eq!(response.extract_total_fees_cents(), 70); // 65 + 2 + 3
    }

    #[test]
    fn test_fee_type_serialization() {
        assert_eq!(
            serde_json::to_string(&FeeType::Commission).unwrap(),
            "\"COMMISSION\""
        );
        assert_eq!(
            serde_json::to_string(&FeeType::SecFee).unwrap(),
            "\"SEC_FEE\""
        );
        assert_eq!(
            serde_json::to_string(&FeeType::TafFee).unwrap(),
            "\"TAF_FEE\""
        );

        let deserialized: FeeType = serde_json::from_str("\"SEC_FEE\"").unwrap();
        assert_eq!(deserialized, FeeType::SecFee);
    }

    #[test]
    fn test_commission_and_fee_parsing_from_json() {
        let json_response = r#"{
            "commission": {
                "commissionLegs": [{
                    "commissionValues": [{
                        "value": 0.65,
                        "type": "COMMISSION"
                    }]
                }]
            },
            "fee": {
                "feeLegs": [{
                    "feeValues": [{
                        "value": 0.01,
                        "type": "SEC_FEE"
                    }]
                }]
            },
            "trueCommission": 0.65
        }"#;

        let parsed: CommissionAndFee = serde_json::from_str(json_response).unwrap();

        assert!(parsed.commission.is_some());
        assert!(parsed.fee.is_some());
        assert_eq!(parsed.true_commission, Some(0.65));

        let commission = parsed.commission.unwrap();
        let commission_legs = commission.commission_legs.unwrap();
        assert_eq!(commission_legs.len(), 1);

        let commission_values = commission_legs[0].commission_values.as_ref().unwrap();
        assert_eq!(commission_values.len(), 1);
        assert!((commission_values[0].value - 0.65).abs() < f64::EPSILON);
        assert_eq!(commission_values[0].fee_type, FeeType::Commission);

        let fees = parsed.fee.unwrap();
        let fee_legs = fees.fee_legs.unwrap();
        assert_eq!(fee_legs.len(), 1);

        let fee_values = fee_legs[0].fee_values.as_ref().unwrap();
        assert_eq!(fee_values.len(), 1);
        assert!((fee_values[0].value - 0.01).abs() < f64::EPSILON);
        assert_eq!(fee_values[0].fee_type, FeeType::SecFee);
    }

    #[test]
    fn test_price_calculation_from_order_activity_collection() {
        let response_json = r#"{
            "status": "FILLED",
            "filledQuantity": 2.0,
            "remainingQuantity": 0.0,
            "orderActivityCollection": [{
                "activityType": "EXECUTION",
                "executionLegs": [{
                    "quantity": 1.0,
                    "price": 100.50
                }, {
                    "quantity": 1.0,
                    "price": 101.50
                }]
            }]
        }"#;

        let parsed: OrderStatusResponse = serde_json::from_str(response_json)
            .expect("Should parse response with orderActivityCollection");

        assert_eq!(parsed.status, Some(OrderStatus::Filled));
        assert!((parsed.filled_quantity.unwrap() - 2.0).abs() < f64::EPSILON);
        assert!(parsed.remaining_quantity.unwrap().abs() < f64::EPSILON);

        let avg_price = parsed.calculate_weighted_average_price();
        assert!(avg_price.is_some());
        assert!((avg_price.unwrap() - 101.0).abs() < f64::EPSILON);
    }
}
