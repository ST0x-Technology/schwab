use super::HasOrderStatus;

/// Order status lifecycle enum for database storage (matches CHECK constraint pattern)
///
/// Represents the lifecycle of an order from creation through completion:
/// - `Pending`: Order created in our system but not yet sent to broker
/// - `Submitted`: Order sent to broker and acknowledged (broker is working on it)
/// - `Filled`: Order successfully completed by broker
/// - `Failed`: Order rejected, canceled, expired, or otherwise terminal without filling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    /// Order exists in our system but hasn't been sent to the broker yet
    Pending,

    /// Order has been sent to and acknowledged by the broker (actively being worked)
    Submitted,

    /// Order has been completely filled by the broker
    Filled,

    /// Order terminated without filling (rejected, canceled, expired, etc.)
    Failed,
}

impl OrderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Submitted => "SUBMITTED",
            Self::Filled => "FILLED",
            Self::Failed => "FAILED",
        }
    }
}

impl HasOrderStatus for OrderStatus {
    fn status_str(&self) -> &'static str {
        self.as_str()
    }
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseOrderStatusError {
    #[error("Invalid order status: '{0}'. Expected one of: PENDING, SUBMITTED, FILLED, FAILED")]
    InvalidStatus(String),
}

impl std::str::FromStr for OrderStatus {
    type Err = ParseOrderStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "PENDING" => Ok(Self::Pending),
            "SUBMITTED" => Ok(Self::Submitted),
            "FILLED" => Ok(Self::Filled),
            "FAILED" => Ok(Self::Failed),
            _ => Err(ParseOrderStatusError::InvalidStatus(s.to_string())),
        }
    }
}
