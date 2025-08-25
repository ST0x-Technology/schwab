const SCHWAB_MINIMUM_WHOLE_SHARES: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionType {
    Long,
    Short,
}

/// Handles position tracking and threshold checking logic.
/// Separated from TradeAccumulator to follow single responsibility principle.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionCalculator {
    pub net_position: f64,
    pub accumulated_long: f64,
    pub accumulated_short: f64,
}

impl Default for PositionCalculator {
    fn default() -> Self {
        Self::new()
    }
}

impl PositionCalculator {
    pub const fn new() -> Self {
        Self {
            net_position: 0.0,
            accumulated_long: 0.0,
            accumulated_short: 0.0,
        }
    }

    pub const fn with_positions(
        net_position: f64,
        accumulated_long: f64,
        accumulated_short: f64,
    ) -> Self {
        Self {
            net_position,
            accumulated_long,
            accumulated_short,
        }
    }

    pub fn should_execute_long(&self) -> bool {
        self.accumulated_long >= SCHWAB_MINIMUM_WHOLE_SHARES
    }

    pub fn should_execute_short(&self) -> bool {
        self.accumulated_short >= SCHWAB_MINIMUM_WHOLE_SHARES
    }

    pub fn determine_execution_type(&self) -> Option<ExecutionType> {
        if self.should_execute_long() {
            Some(ExecutionType::Long)
        } else if self.should_execute_short() {
            Some(ExecutionType::Short)
        } else {
            None
        }
    }

    pub fn add_trade(&mut self, amount: f64, direction: ExecutionType) {
        match direction {
            ExecutionType::Long => {
                // Buy on Schwab (offset short position) = accumulate for long execution
                self.accumulated_long += amount;
                self.net_position += amount;
            }
            ExecutionType::Short => {
                // Sell on Schwab (offset long position) = accumulate for short execution
                self.accumulated_short += amount;
                self.net_position -= amount;
            }
        }
    }

    pub fn reduce_accumulation(&mut self, execution_type: ExecutionType, shares: u64) {
        // precision loss only occurs beyond 2^53 shares (unrealistic for equity trading)
        #[allow(clippy::cast_precision_loss)]
        let shares_amount = shares as f64;

        match execution_type {
            ExecutionType::Long => {
                self.accumulated_long -= shares_amount;
            }
            ExecutionType::Short => {
                self.accumulated_short -= shares_amount;
            }
        }
    }

    pub const fn get_accumulated_amount(&self, execution_type: ExecutionType) -> f64 {
        match execution_type {
            ExecutionType::Long => self.accumulated_long,
            ExecutionType::Short => self.accumulated_short,
        }
    }

    pub fn calculate_executable_shares(&self, execution_type: ExecutionType) -> u64 {
        let accumulated_amount = self.get_accumulated_amount(execution_type);
        shares_from_amount_floor(accumulated_amount)
    }
}

/// Converts accumulated amount to whole shares using floor (conservative approach).
///
/// Uses floor rather than round to ensure we never execute more shares than
/// we have accumulated fractional amounts for.
fn shares_from_amount_floor(amount: f64) -> u64 {
    if amount < 0.0 {
        0 // Negative amounts result in 0 shares
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            amount.floor() as u64 // Safe: floor() removes fractional part, negative case handled above
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_calculator_new() {
        let calc = PositionCalculator::new();
        assert!((calc.net_position - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_long - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_should_execute_below_threshold() {
        let calc = PositionCalculator::with_positions(0.0, 0.5, 0.8);
        assert!(!calc.should_execute_long());
        assert!(!calc.should_execute_short());
        assert!(calc.determine_execution_type().is_none());
    }

    #[test]
    fn test_should_execute_above_threshold() {
        let calc = PositionCalculator::with_positions(0.0, 1.5, 2.0);
        assert!(calc.should_execute_long());
        assert!(calc.should_execute_short());
        assert_eq!(calc.determine_execution_type(), Some(ExecutionType::Long));
    }

    #[test]
    fn test_add_trade_long_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(1.5, ExecutionType::Long); // Buy on Schwab = accumulate for long execution
        assert!((calc.accumulated_long - 1.5).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 0.0).abs() < f64::EPSILON);
        assert!((calc.net_position - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_short_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(2.0, ExecutionType::Short); // Sell on Schwab = accumulate for short execution
        assert!((calc.accumulated_long - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON);
        assert!((calc.net_position - (-2.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_zero_amount() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(0.0, ExecutionType::Long); // Zero amount but still affects direction
        assert!((calc.accumulated_long - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 0.0).abs() < f64::EPSILON);
        assert!((calc.net_position - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_mixed_directions() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(1.5, ExecutionType::Long); // Long accumulation
        calc.add_trade(2.0, ExecutionType::Short); // Short accumulation
        calc.add_trade(0.3, ExecutionType::Long); // More long accumulation

        assert!((calc.accumulated_long - 1.8).abs() < f64::EPSILON); // 1.5 + 0.3
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON); // 2.0
        assert!((calc.net_position - (-0.2)).abs() < f64::EPSILON); // 1.5 - 2.0 + 0.3 = -0.2
    }

    #[test]
    fn test_reduce_accumulation() {
        let mut calc = PositionCalculator::with_positions(0.0, 2.5, 3.0);
        calc.reduce_accumulation(ExecutionType::Long, 2);
        assert!((calc.accumulated_long - 0.5).abs() < f64::EPSILON);

        calc.reduce_accumulation(ExecutionType::Short, 1);
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_executable_shares() {
        let calc = PositionCalculator::with_positions(0.0, 2.7, 3.2);
        assert_eq!(calc.calculate_executable_shares(ExecutionType::Long), 2);
        assert_eq!(calc.calculate_executable_shares(ExecutionType::Short), 3);
    }
}
