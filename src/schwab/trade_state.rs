use chrono::{DateTime, Utc};

use super::{TradeStatus, price_cents_from_db_i64};
use crate::error::{OnChainError, PersistenceError};

/// Database fields extracted from TradeState for storage
#[derive(Debug)]
pub(crate) struct TradeStateDbFields {
    pub(crate) order_id: Option<String>,
    pub(crate) price_cents: Option<i64>,
    pub(crate) executed_at: Option<chrono::NaiveDateTime>,
    pub(crate) commission_cents: Option<i64>,
    pub(crate) sec_fee_cents: Option<i64>,
    pub(crate) taf_fee_cents: Option<i64>,
    pub(crate) other_fees_cents: Option<i64>,
}

impl TradeStateDbFields {
    /// Calculate total fees from component fees at runtime - ESSENTIAL for P&L calculations
    pub(crate) fn total_fees_cents(&self) -> Option<i64> {
        match (
            self.commission_cents,
            self.sec_fee_cents,
            self.taf_fee_cents,
            self.other_fees_cents,
        ) {
            (Some(c), Some(s), Some(t), Some(o)) => Some(c + s + t + o),
            _ => None,
        }
    }
}

// Stateful enum with associated data for runtime use
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeState {
    Pending,
    Submitted {
        order_id: String,
    },
    Filled {
        executed_at: DateTime<Utc>,
        order_id: String,
        price_cents: u64,
        commission_cents: u64,
        sec_fee_cents: u64,
        taf_fee_cents: u64,
        other_fees_cents: u64,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error_reason: Option<String>,
    },
}

/// Trait for types that can be converted to a status string for database queries
pub(crate) trait HasTradeStatus {
    fn status_str(&self) -> &'static str;
}

impl HasTradeStatus for TradeStatus {
    fn status_str(&self) -> &'static str {
        self.as_str()
    }
}

impl HasTradeStatus for TradeState {
    fn status_str(&self) -> &'static str {
        self.status().as_str()
    }
}

impl TradeState {
    pub const fn status(&self) -> TradeStatus {
        match self {
            Self::Pending => TradeStatus::Pending,
            Self::Submitted { .. } => TradeStatus::Submitted,
            Self::Filled { .. } => TradeStatus::Filled,
            Self::Failed { .. } => TradeStatus::Failed,
        }
    }

    /// Calculate total fees from component fees for Filled state - ESSENTIAL for P&L calculations
    pub fn total_fees_cents(&self) -> Option<u64> {
        match self {
            Self::Filled {
                commission_cents,
                sec_fee_cents,
                taf_fee_cents,
                other_fees_cents,
                ..
            } => Some(commission_cents + sec_fee_cents + taf_fee_cents + other_fees_cents),
            _ => None,
        }
    }

    /// Converts database row data to a TradeState instance with proper validation.
    /// This centralizes the conversion logic and ensures database consistency.
    pub(crate) fn from_db_row(
        status: TradeStatus,
        db_fields: TradeStateDbFields,
    ) -> Result<Self, OnChainError> {
        match status {
            TradeStatus::Pending => Ok(Self::Pending),
            TradeStatus::Submitted => {
                let order_id = db_fields.order_id.ok_or_else(|| {
                    OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                        "SUBMITTED requires order_id".to_string(),
                    ))
                })?;
                Ok(Self::Submitted { order_id })
            }
            TradeStatus::Filled => {
                let order_id = db_fields.order_id.ok_or_else(|| {
                    OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                        "FILLED requires order_id".to_string(),
                    ))
                })?;
                let price_cents = db_fields.price_cents.ok_or_else(|| {
                    OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                        "FILLED requires price_cents".to_string(),
                    ))
                })?;
                let executed_at = db_fields.executed_at.ok_or_else(|| {
                    OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                        "FILLED requires executed_at".to_string(),
                    ))
                })?;
                Ok(Self::Filled {
                    executed_at: DateTime::<Utc>::from_naive_utc_and_offset(executed_at, Utc),
                    order_id,
                    price_cents: price_cents_from_db_i64(price_cents)?,
                    commission_cents: price_cents_from_db_i64(
                        db_fields.commission_cents.unwrap_or(0),
                    )?,
                    sec_fee_cents: price_cents_from_db_i64(db_fields.sec_fee_cents.unwrap_or(0))?,
                    taf_fee_cents: price_cents_from_db_i64(db_fields.taf_fee_cents.unwrap_or(0))?,
                    other_fees_cents: price_cents_from_db_i64(
                        db_fields.other_fees_cents.unwrap_or(0),
                    )?,
                })
            }
            TradeStatus::Failed => {
                let failed_at = db_fields.executed_at.ok_or_else(|| {
                    OnChainError::Persistence(PersistenceError::InvalidTradeStatus(
                        "FAILED requires executed_at timestamp".to_string(),
                    ))
                })?;
                Ok(Self::Failed {
                    failed_at: DateTime::<Utc>::from_naive_utc_and_offset(failed_at, Utc),
                    error_reason: None, // We don't store error_reason in database yet
                })
            }
        }
    }

    /// Extracts database-compatible values from TradeState for storage.
    /// Returns TradeStateDbFields with all relevant data for database persistence.
    pub(crate) fn to_db_fields(&self) -> Result<TradeStateDbFields, OnChainError> {
        match self {
            Self::Pending => Ok(TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            }),
            Self::Submitted { order_id } => Ok(TradeStateDbFields {
                order_id: Some(order_id.clone()),
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            }),
            Self::Filled {
                executed_at,
                order_id,
                price_cents,
                commission_cents,
                sec_fee_cents,
                taf_fee_cents,
                other_fees_cents,
            } => Ok(TradeStateDbFields {
                order_id: Some(order_id.clone()),
                price_cents: Some(u64_to_i64_exact(*price_cents)?),
                executed_at: Some(executed_at.naive_utc()),
                commission_cents: Some(u64_to_i64_exact(*commission_cents)?),
                sec_fee_cents: Some(u64_to_i64_exact(*sec_fee_cents)?),
                taf_fee_cents: Some(u64_to_i64_exact(*taf_fee_cents)?),
                other_fees_cents: Some(u64_to_i64_exact(*other_fees_cents)?),
            }),
            Self::Failed {
                failed_at,
                error_reason: _,
            } => Ok(TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: Some(failed_at.naive_utc()),
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            }),
        }
    }
}

/// Converts u64 to i64 for database storage with exact conversion.
/// NEVER silently changes amounts - returns error if conversion would lose data.
/// This is critical for financial applications where data integrity is paramount.
fn u64_to_i64_exact(value: u64) -> Result<i64, OnChainError> {
    if value > i64::MAX as u64 {
        Err(OnChainError::Persistence(
            PersistenceError::InvalidTradeStatus(format!(
                "Value {value} exceeds maximum i64 range - conversion would lose data"
            )),
        ))
    } else {
        #[allow(clippy::cast_possible_wrap)]
        Ok(value as i64) // Safe: verified within i64 range
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_from_db_row_pending() {
        let result = TradeState::from_db_row(
            TradeStatus::Pending,
            TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            },
        )
        .unwrap();
        assert_eq!(result, TradeState::Pending);
    }

    #[test]
    fn test_from_db_row_submitted() {
        let result = TradeState::from_db_row(
            TradeStatus::Submitted,
            TradeStateDbFields {
                order_id: Some("1004055538123".to_string()),
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            },
        )
        .unwrap();
        assert_eq!(
            result,
            TradeState::Submitted {
                order_id: "1004055538123".to_string()
            }
        );
    }

    #[test]
    fn test_from_db_row_filled() {
        let timestamp = Utc::now().naive_utc();
        let result = TradeState::from_db_row(
            TradeStatus::Filled,
            TradeStateDbFields {
                order_id: Some("1004055538123".to_string()),
                price_cents: Some(15000),
                executed_at: Some(timestamp),
                commission_cents: Some(65), // commission_cents
                sec_fee_cents: Some(1),     // sec_fee_cents
                taf_fee_cents: Some(2),     // taf_fee_cents
                other_fees_cents: Some(5),  // other_fees_cents
            },
        )
        .unwrap();

        match &result {
            TradeState::Filled {
                executed_at,
                order_id,
                price_cents,
                commission_cents,
                sec_fee_cents,
                taf_fee_cents,
                other_fees_cents,
            } => {
                assert_eq!(order_id, "1004055538123");
                assert_eq!(*price_cents, 15000);
                assert_eq!(executed_at.naive_utc(), timestamp);
                assert_eq!(*commission_cents, 65);
                assert_eq!(*sec_fee_cents, 1);
                assert_eq!(*taf_fee_cents, 2);
                assert_eq!(*other_fees_cents, 5);
            }
            _ => panic!("Expected Filled variant"),
        }
        assert_eq!(result.total_fees_cents(), Some(73));
    }

    #[test]
    fn test_from_db_row_failed() {
        let timestamp = Utc::now().naive_utc();
        let result = TradeState::from_db_row(
            TradeStatus::Failed,
            TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: Some(timestamp),
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            },
        )
        .unwrap();

        match result {
            TradeState::Failed {
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
        let result = TradeState::from_db_row(
            TradeStatus::Submitted,
            TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_order_id() {
        let timestamp = Utc::now().naive_utc();
        let result = TradeState::from_db_row(
            TradeStatus::Filled,
            TradeStateDbFields {
                order_id: None,
                price_cents: Some(15000),
                executed_at: Some(timestamp),
                commission_cents: Some(65),
                sec_fee_cents: Some(1),
                taf_fee_cents: Some(2),
                other_fees_cents: Some(5),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_price_cents() {
        let timestamp = Utc::now().naive_utc();
        let result = TradeState::from_db_row(
            TradeStatus::Filled,
            TradeStateDbFields {
                order_id: Some("1004055538123".to_string()),
                price_cents: None,
                executed_at: Some(timestamp),
                commission_cents: Some(65),
                sec_fee_cents: Some(1),
                taf_fee_cents: Some(2),
                other_fees_cents: Some(5),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_filled_missing_executed_at() {
        let result = TradeState::from_db_row(
            TradeStatus::Filled,
            TradeStateDbFields {
                order_id: Some("1004055538123".to_string()),
                price_cents: Some(15000),
                executed_at: None,
                commission_cents: Some(65),
                sec_fee_cents: Some(1),
                taf_fee_cents: Some(2),
                other_fees_cents: Some(5),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_from_db_row_failed_missing_executed_at() {
        let result = TradeState::from_db_row(
            TradeStatus::Failed,
            TradeStateDbFields {
                order_id: None,
                price_cents: None,
                executed_at: None,
                commission_cents: None,
                sec_fee_cents: None,
                taf_fee_cents: None,
                other_fees_cents: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_to_db_fields_pending() {
        let state = TradeState::Pending;
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, None);
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, None);
        assert_eq!(db_fields.commission_cents, None);
        assert_eq!(db_fields.sec_fee_cents, None);
        assert_eq!(db_fields.taf_fee_cents, None);
        assert_eq!(db_fields.other_fees_cents, None);
        assert_eq!(db_fields.total_fees_cents(), None);
    }

    #[test]
    fn test_to_db_fields_submitted() {
        let state = TradeState::Submitted {
            order_id: "1004055538123".to_string(),
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, Some("1004055538123".to_string()));
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, None);
        assert_eq!(db_fields.commission_cents, None);
        assert_eq!(db_fields.sec_fee_cents, None);
        assert_eq!(db_fields.taf_fee_cents, None);
        assert_eq!(db_fields.other_fees_cents, None);
        assert_eq!(db_fields.total_fees_cents(), None);
    }

    #[test]
    fn test_to_db_fields_filled() {
        let timestamp = Utc::now();
        let state = TradeState::Filled {
            executed_at: timestamp,
            order_id: "1004055538123".to_string(),
            price_cents: 15000,
            commission_cents: 65,
            sec_fee_cents: 1,
            taf_fee_cents: 2,
            other_fees_cents: 5,
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, Some("1004055538123".to_string()));
        assert_eq!(db_fields.price_cents, Some(15000));
        assert_eq!(db_fields.executed_at, Some(timestamp.naive_utc()));
        assert_eq!(db_fields.commission_cents, Some(65));
        assert_eq!(db_fields.sec_fee_cents, Some(1));
        assert_eq!(db_fields.taf_fee_cents, Some(2));
        assert_eq!(db_fields.other_fees_cents, Some(5));
        assert_eq!(db_fields.total_fees_cents(), Some(73));
        assert_eq!(state.total_fees_cents(), Some(73));
    }

    #[test]
    fn test_to_db_fields_failed() {
        let timestamp = Utc::now();
        let state = TradeState::Failed {
            failed_at: timestamp,
            error_reason: Some("Test error".to_string()),
        };
        let db_fields = state.to_db_fields().unwrap();
        assert_eq!(db_fields.order_id, None);
        assert_eq!(db_fields.price_cents, None);
        assert_eq!(db_fields.executed_at, Some(timestamp.naive_utc()));
        assert_eq!(db_fields.commission_cents, None);
        assert_eq!(db_fields.sec_fee_cents, None);
        assert_eq!(db_fields.taf_fee_cents, None);
        assert_eq!(db_fields.other_fees_cents, None);
        assert_eq!(db_fields.total_fees_cents(), None);
    }

    #[test]
    fn test_status_extraction() {
        assert_eq!(TradeState::Pending.status(), TradeStatus::Pending);
        assert_eq!(
            TradeState::Submitted {
                order_id: "1004055538123".to_string()
            }
            .status(),
            TradeStatus::Submitted
        );
        assert_eq!(
            TradeState::Filled {
                executed_at: Utc::now(),
                order_id: "1004055538123".to_string(),
                price_cents: 15000,
                commission_cents: 65,
                sec_fee_cents: 1,
                taf_fee_cents: 2,
                other_fees_cents: 5,
            }
            .status(),
            TradeStatus::Filled
        );
        assert_eq!(
            TradeState::Failed {
                failed_at: Utc::now(),
                error_reason: None,
            }
            .status(),
            TradeStatus::Failed
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
}
