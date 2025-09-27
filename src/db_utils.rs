use crate::error::OnChainError;
use st0x_broker::PersistenceError;

/// Converts database row data i64 to u64 shares with validation.
/// Returns error if the database value is negative.
pub(crate) fn shares_from_db_i64(db_value: i64) -> Result<u64, OnChainError> {
    if db_value < 0 {
        Err(OnChainError::Persistence(
            PersistenceError::InvalidShareQuantity(db_value),
        ))
    } else {
        #[allow(clippy::cast_sign_loss)]
        Ok(db_value as u64)
    }
}
