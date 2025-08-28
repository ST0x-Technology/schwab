use serde::{Deserialize, Serialize};

use super::price_cents_from_db_i64;
use crate::error::OnChainError;

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

/// Individual execution leg representing a specific fill
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionLeg {
    pub execution_id: Option<String>,
    pub quantity: f64,
    pub price: f64,
    pub time: Option<String>,
}

/// Order status response from Schwab API
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OrderStatusResponse {
    pub order_id: Option<String>,
    pub status: OrderStatus,
    pub filled_quantity: f64,
    pub remaining_quantity: f64,
    pub execution_legs: Vec<ExecutionLeg>,
    pub entered_time: Option<String>,
    pub close_time: Option<String>,
}

// TODO: Remove #[allow(dead_code)] when integrating order poller in Section 5
#[allow(dead_code)]
impl OrderStatusResponse {
    /// Calculate weighted average fill price from execution legs
    pub fn calculate_weighted_average_price(&self) -> Option<f64> {
        if self.execution_legs.is_empty() {
            return None;
        }

        let mut total_value = 0.0;
        let mut total_quantity = 0.0;

        for leg in &self.execution_legs {
            total_value += leg.price * leg.quantity;
            total_quantity += leg.quantity;
        }

        if total_quantity > 0.0 {
            Some(total_value / total_quantity)
        } else {
            None
        }
    }

    /// Convert price to cents for database storage
    pub fn price_in_cents(&self) -> Result<Option<u64>, OnChainError> {
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
    pub const fn is_filled(&self) -> bool {
        matches!(self.status, OrderStatus::Filled)
    }

    /// Check if order is still pending/working
    pub const fn is_pending(&self) -> bool {
        matches!(
            self.status,
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
    }

    /// Check if order was canceled or rejected
    pub const fn is_terminal_failure(&self) -> bool {
        matches!(
            self.status,
            OrderStatus::Canceled | OrderStatus::Rejected | OrderStatus::Expired
        )
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
    fn test_execution_leg_serialization() {
        let leg = ExecutionLeg {
            execution_id: Some("EXEC123".to_string()),
            quantity: 100.0,
            price: 150.25,
            time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        let json = serde_json::to_string(&leg).unwrap();
        let deserialized: ExecutionLeg = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, leg);
    }

    #[test]
    fn test_order_status_response_serialization() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 100.0,
            remaining_quantity: 0.0,
            execution_legs: vec![ExecutionLeg {
                execution_id: Some("EXEC123".to_string()),
                quantity: 100.0,
                price: 150.25,
                time: Some("2023-10-15T10:30:00Z".to_string()),
            }],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: OrderStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, response);
    }

    #[test]
    fn test_calculate_weighted_average_price_single_leg() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 100.0,
            remaining_quantity: 0.0,
            execution_legs: vec![ExecutionLeg {
                execution_id: Some("EXEC123".to_string()),
                quantity: 100.0,
                price: 150.25,
                time: Some("2023-10-15T10:30:00Z".to_string()),
            }],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        assert_eq!(response.calculate_weighted_average_price(), Some(150.25));
    }

    #[test]
    fn test_calculate_weighted_average_price_multiple_legs() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 200.0,
            remaining_quantity: 0.0,
            execution_legs: vec![
                ExecutionLeg {
                    execution_id: Some("EXEC123".to_string()),
                    quantity: 100.0,
                    price: 150.00,
                    time: Some("2023-10-15T10:30:00Z".to_string()),
                },
                ExecutionLeg {
                    execution_id: Some("EXEC124".to_string()),
                    quantity: 100.0,
                    price: 151.00,
                    time: Some("2023-10-15T10:30:10Z".to_string()),
                },
            ],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:10Z".to_string()),
        };

        assert_eq!(response.calculate_weighted_average_price(), Some(150.5));
    }

    #[test]
    fn test_calculate_weighted_average_price_weighted() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 300.0,
            remaining_quantity: 0.0,
            execution_legs: vec![
                ExecutionLeg {
                    execution_id: Some("EXEC123".to_string()),
                    quantity: 200.0, // 2/3 of total
                    price: 150.00,
                    time: Some("2023-10-15T10:30:00Z".to_string()),
                },
                ExecutionLeg {
                    execution_id: Some("EXEC124".to_string()),
                    quantity: 100.0, // 1/3 of total
                    price: 153.00,
                    time: Some("2023-10-15T10:30:10Z".to_string()),
                },
            ],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:10Z".to_string()),
        };

        // (200 * 150.00 + 100 * 153.00) / 300 = (30000 + 15300) / 300 = 151.00
        assert_eq!(response.calculate_weighted_average_price(), Some(151.0));
    }

    #[test]
    fn test_calculate_weighted_average_price_empty_legs() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Working,
            filled_quantity: 0.0,
            remaining_quantity: 100.0,
            execution_legs: vec![],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), None);
    }

    #[test]
    fn test_price_in_cents_conversion() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 100.0,
            remaining_quantity: 0.0,
            execution_legs: vec![ExecutionLeg {
                execution_id: Some("EXEC123".to_string()),
                quantity: 100.0,
                price: 150.25,
                time: Some("2023-10-15T10:30:00Z".to_string()),
            }],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        assert_eq!(response.price_in_cents().unwrap(), Some(15025));
    }

    #[test]
    fn test_price_in_cents_no_executions() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Working,
            filled_quantity: 0.0,
            remaining_quantity: 100.0,
            execution_legs: vec![],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
        };

        assert_eq!(response.price_in_cents().unwrap(), None);
    }

    #[test]
    fn test_price_in_cents_rounding() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 100.0,
            remaining_quantity: 0.0,
            execution_legs: vec![ExecutionLeg {
                execution_id: Some("EXEC123".to_string()),
                quantity: 100.0,
                price: 150.254, // Should round to 150.25
                time: Some("2023-10-15T10:30:00Z".to_string()),
            }],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        assert_eq!(response.price_in_cents().unwrap(), Some(15025));
    }

    #[test]
    fn test_is_filled() {
        let mut response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Filled,
            filled_quantity: 100.0,
            remaining_quantity: 0.0,
            execution_legs: vec![],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: Some("2023-10-15T10:30:00Z".to_string()),
        };

        assert!(response.is_filled());

        response.status = OrderStatus::Working;
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
                order_id: Some("ORDER123".to_string()),
                status,
                filled_quantity: 0.0,
                remaining_quantity: 100.0,
                execution_legs: vec![],
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: None,
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
                order_id: Some("ORDER123".to_string()),
                status,
                filled_quantity: 100.0,
                remaining_quantity: 0.0,
                execution_legs: vec![],
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: Some("2023-10-15T10:30:00Z".to_string()),
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
                order_id: Some("ORDER123".to_string()),
                status,
                filled_quantity: 0.0,
                remaining_quantity: 100.0,
                execution_legs: vec![],
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: Some("2023-10-15T10:30:00Z".to_string()),
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
                order_id: Some("ORDER123".to_string()),
                status,
                filled_quantity: 0.0,
                remaining_quantity: 100.0,
                execution_legs: vec![],
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: None,
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
            "orderId": "ORDER12345",
            "status": "FILLED",
            "filledQuantity": 200.0,
            "remainingQuantity": 0.0,
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
            ],
            "enteredTime": "2023-10-15T10:25:00Z",
            "closeTime": "2023-10-15T10:30:05Z"
        }
        "#;

        let response: OrderStatusResponse = serde_json::from_str(json_response).unwrap();

        assert_eq!(response.order_id, Some("ORDER12345".to_string()));
        assert_eq!(response.status, OrderStatus::Filled);
        assert!((response.filled_quantity - 200.0).abs() < f64::EPSILON);
        assert!(response.remaining_quantity.abs() < f64::EPSILON);
        assert_eq!(response.execution_legs.len(), 2);

        // Test weighted average: (150 * 100.25 + 50 * 100.75) / 200 = (15037.5 + 5037.5) / 200 = 100.375
        let avg_price = response.calculate_weighted_average_price().unwrap();
        assert!((avg_price - 100.375).abs() < f64::EPSILON);
        assert_eq!(response.price_in_cents().unwrap(), Some(10038)); // Rounded
    }

    #[test]
    fn test_edge_case_zero_quantity_legs() {
        let response = OrderStatusResponse {
            order_id: Some("ORDER123".to_string()),
            status: OrderStatus::Working,
            filled_quantity: 0.0,
            remaining_quantity: 100.0,
            execution_legs: vec![ExecutionLeg {
                execution_id: Some("EXEC123".to_string()),
                quantity: 0.0, // Zero quantity
                price: 150.25,
                time: Some("2023-10-15T10:30:00Z".to_string()),
            }],
            entered_time: Some("2023-10-15T10:25:00Z".to_string()),
            close_time: None,
        };

        assert_eq!(response.calculate_weighted_average_price(), None);
        assert_eq!(response.price_in_cents().unwrap(), None);
    }
}
