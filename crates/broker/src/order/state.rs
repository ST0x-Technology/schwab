use chrono::{DateTime, Utc};

use super::OrderStatus;
use crate::BrokerError;

/// Database fields extracted from OrderState for storage
#[derive(Debug)]
pub(crate) struct OrderStateDbFields {
    pub(crate) order_id: Option<String>,
    pub(crate) price_cents: Option<i64>,
    pub(crate) executed_at: Option<chrono::NaiveDateTime>,
}

// Stateful enum with associated data for runtime use
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderState {
    Pending,
    Submitted {
        order_id: String,
    },
    Filled {
        executed_at: DateTime<Utc>,
        order_id: String,
        price_cents: u64,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error_reason: Option<String>,
    },
}

/// Trait for types that can be converted to a status string for database queries
impl super::HasOrderStatus for OrderState {
    fn status_str(&self) -> &'static str {
        self.status().as_str()
    }
}

impl OrderState {
    pub const fn status(&self) -> OrderStatus {
        match self {
            Self::Pending => OrderStatus::Pending,
            Self::Submitted { .. } => OrderStatus::Submitted,
            Self::Filled { .. } => OrderStatus::Filled,
            Self::Failed { .. } => OrderStatus::Failed,
        }
    }

    /// Converts database row data to an OrderState instance with proper validation.
    /// This centralizes the conversion logic and ensures database consistency.
    pub fn from_db_row(
        status: OrderStatus,
        order_id: Option<String>,
        price_cents: Option<i64>,
        executed_at: Option<chrono::NaiveDateTime>,
    ) -> Result<Self, BrokerError> {
        match status {
            OrderStatus::Pending => Ok(Self::Pending),
            OrderStatus::Submitted => {
                let order_id = order_id.ok_or_else(|| BrokerError::InvalidOrder {
                    reason: "SUBMITTED requires order_id".to_string(),
                })?;
                Ok(Self::Submitted { order_id })
            }
            OrderStatus::Filled => {
                let order_id = order_id.ok_or_else(|| BrokerError::InvalidOrder {
                    reason: "FILLED requires order_id".to_string(),
                })?;
                let price_cents = price_cents.ok_or_else(|| BrokerError::InvalidOrder {
                    reason: "FILLED requires price_cents".to_string(),
                })?;
                let executed_at = executed_at.ok_or_else(|| BrokerError::InvalidOrder {
                    reason: "FILLED requires executed_at".to_string(),
                })?;
                Ok(Self::Filled {
                    executed_at: DateTime::<Utc>::from_naive_utc_and_offset(executed_at, Utc),
                    order_id,
                    price_cents: price_cents_from_db_i64(price_cents)?,
                })
            }
            OrderStatus::Failed => {
                let failed_at = executed_at.ok_or_else(|| BrokerError::InvalidOrder {
                    reason: "FAILED requires executed_at timestamp".to_string(),
                })?;
                Ok(Self::Failed {
                    failed_at: DateTime::<Utc>::from_naive_utc_and_offset(failed_at, Utc),
                    error_reason: None, // We don't store error_reason in database yet
                })
            }
        }
    }

    /// Extracts database-compatible values from OrderState for storage.
    /// Returns (order_id, price_cents_i64, executed_at) tuple.
    pub fn to_db_fields(&self) -> Result<OrderStateDbFields, BrokerError> {
        match self {
            Self::Pending => Ok(OrderStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: None,
            }),
            Self::Submitted { order_id } => Ok(OrderStateDbFields {
                order_id: Some(order_id.clone()),
                price_cents: None,
                executed_at: None,
            }),
            Self::Filled {
                executed_at,
                order_id,
                price_cents,
            } => Ok(OrderStateDbFields {
                order_id: Some(order_id.clone()),
                price_cents: Some(u64_to_i64_exact(*price_cents)?),
                executed_at: Some(executed_at.naive_utc()),
            }),
            Self::Failed {
                failed_at,
                error_reason: _,
            } => Ok(OrderStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: Some(failed_at.naive_utc()),
            }),
        }
    }
}

/// Converts u64 to i64 for database storage with exact conversion.
/// NEVER silently changes amounts - returns error if conversion would lose data.
/// This is critical for financial applications where data integrity is paramount.
fn u64_to_i64_exact(value: u64) -> Result<i64, BrokerError> {
    if value > i64::MAX as u64 {
        Err(BrokerError::InvalidOrder {
            reason: format!("Value {value} exceeds maximum i64 range - conversion would lose data"),
        })
    } else {
        #[allow(clippy::cast_possible_wrap)]
        Ok(value as i64) // Safe: verified within i64 range
    }
}

/// Converts i64 from database to u64 for application use with exact conversion.
/// NEVER silently changes amounts - returns error if conversion would lose data.
fn price_cents_from_db_i64(value: i64) -> Result<u64, BrokerError> {
    if value < 0 {
        Err(BrokerError::InvalidOrder {
            reason: format!("Negative price_cents value {value} is invalid"),
        })
    } else {
        Ok(value as u64) // Safe: verified non-negative
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_from_db_row_pending() {
        let result = OrderState::from_db_row(OrderStatus::Pending, None, None, None).unwrap();
        assert_eq!(result, OrderState::Pending);
    }

    #[test]
    fn test_from_db_row_submitted() {
        let result = OrderState::from_db_row(
            OrderStatus::Submitted,
            Some("ORDER123".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            result,
            OrderState::Submitted {
                order_id: "ORDER123".to_string()
            }
        );
    }

    #[test]
    fn test_from_db_row_filled() {
        let timestamp = Utc::now().naive_utc();
        let result = OrderState::from_db_row(
            OrderStatus::Filled,
            Some("ORDER123".to_string()),
            Some(15000),
            Some(timestamp),
        )
        .unwrap();

        match result {
            OrderState::Filled {
                executed_at,
                order_id,
                price_cents,
            } => {
                assert_eq!(order_id, "ORDER123");
                assert_eq!(price_cents, 15000);
                assert_eq!(executed_at.naive_utc(), timestamp);
            }
            _ => panic!("Expected Filled variant"),
        }
    }

    #[test]
    fn test_from_db_row_failed() {
        let timestamp = Utc::now().naive_utc();
        let result =
            OrderState::from_db_row(OrderStatus::Failed, None, None, Some(timestamp)).unwrap();

        match result {
            OrderState::Failed {
                failed_at,
                error_reason,
            } => {
                assert_eq!(failed_at.naive_utc(), timestamp);
                assert_eq!(error_reason, None);
            }
            _ => panic!("Expected Failed variant"),
        }
    }

    #[test]
    fn test_from_db_row_submitted_missing_order_id() {
        let result = OrderState::from_db_row(OrderStatus::Submitted, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_order_id() {
        let timestamp = Utc::now().naive_utc();
        let result =
            OrderState::from_db_row(OrderStatus::Filled, None, Some(15000), Some(timestamp));
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_price_cents() {
        let timestamp = Utc::now().naive_utc();
        let result = OrderState::from_db_row(
            OrderStatus::Filled,
            Some("ORDER123".to_string()),
            None,
            Some(timestamp),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_executed_at() {
        let result = OrderState::from_db_row(
            OrderStatus::Filled,
            Some("ORDER123".to_string()),
            Some(15000),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_failed_missing_executed_at() {
        let result = OrderState::from_db_row(OrderStatus::Failed, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_to_db_fields_pending() {
        let state = OrderState::Pending;
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, None);
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, None);
    }

    #[test]
    fn test_to_db_fields_submitted() {
        let state = OrderState::Submitted {
            order_id: "ORDER123".to_string(),
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, Some("ORDER123".to_string()));
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, None);
    }

    #[test]
    fn test_to_db_fields_filled() {
        let timestamp = Utc::now();
        let state = OrderState::Filled {
            executed_at: timestamp,
            order_id: "ORDER123".to_string(),
            price_cents: 15000,
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, Some("ORDER123".to_string()));
        assert_eq!(db_fields.price_cents, Some(15000));
        assert_eq!(db_fields.executed_at, Some(timestamp.naive_utc()));
    }

    #[test]
    fn test_to_db_fields_failed() {
        let timestamp = Utc::now();
        let state = OrderState::Failed {
            failed_at: timestamp,
            error_reason: Some("Test error".to_string()),
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, None);
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, Some(timestamp.naive_utc()));
    }

    #[test]
    fn test_status_extraction() {
        assert_eq!(OrderState::Pending.status(), OrderStatus::Pending);
        assert_eq!(
            OrderState::Submitted {
                order_id: "ORDER123".to_string()
            }
            .status(),
            OrderStatus::Submitted
        );
        assert_eq!(
            OrderState::Filled {
                executed_at: Utc::now(),
                order_id: "ORDER123".to_string(),
                price_cents: 15000,
            }
            .status(),
            OrderStatus::Filled
        );
        assert_eq!(
            OrderState::Failed {
                failed_at: Utc::now(),
                error_reason: None,
            }
            .status(),
            OrderStatus::Failed
        );
    }

    #[test]
    fn test_u64_to_i64_exact_normal_values() {
        assert_eq!(u64_to_i64_exact(0).unwrap(), 0);
        assert_eq!(u64_to_i64_exact(100).unwrap(), 100);
        assert_eq!(u64_to_i64_exact(15000).unwrap(), 15000);
    }

    #[test]
    fn test_u64_to_i64_exact_max_value() {
        assert_eq!(u64_to_i64_exact(i64::MAX as u64).unwrap(), i64::MAX);
    }

    #[test]
    fn test_u64_to_i64_exact_overflow() {
        let overflow_value = (i64::MAX as u64) + 1;
        let result = u64_to_i64_exact(overflow_value);
        assert!(result.is_err()); // MUST fail, never silently change amounts
    }

    #[test]
    fn test_price_cents_from_db_i64_positive() {
        assert_eq!(price_cents_from_db_i64(0).unwrap(), 0);
        assert_eq!(price_cents_from_db_i64(15000).unwrap(), 15000);
        assert_eq!(price_cents_from_db_i64(i64::MAX).unwrap(), i64::MAX as u64);
    }

    #[test]
    fn test_price_cents_from_db_i64_negative() {
        let result = price_cents_from_db_i64(-1);
        assert!(result.is_err()); // MUST fail for negative values
    }
}
