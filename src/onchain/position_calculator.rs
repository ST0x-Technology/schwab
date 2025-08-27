use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

const SCHWAB_MINIMUM_WHOLE_SHARES: Decimal = Decimal::ONE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionType {
    Long,
    Short,
}

/// Handles position tracking and threshold checking logic.
/// Separated from TradeAccumulator to follow single responsibility principle.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionCalculator {
    pub net_position: Decimal,
    pub accumulated_long: Decimal,
    pub accumulated_short: Decimal,
}

impl Default for PositionCalculator {
    fn default() -> Self {
        Self::new()
    }
}

impl PositionCalculator {
    pub const fn new() -> Self {
        Self {
            net_position: Decimal::ZERO,
            accumulated_long: Decimal::ZERO,
            accumulated_short: Decimal::ZERO,
        }
    }

    pub const fn with_positions(
        net_position: Decimal,
        accumulated_long: Decimal,
        accumulated_short: Decimal,
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

    pub fn add_trade(&mut self, amount: Decimal, direction: ExecutionType) {
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
        let shares_amount = Decimal::from(shares);

        match execution_type {
            ExecutionType::Long => {
                self.accumulated_long -= shares_amount;
            }
            ExecutionType::Short => {
                self.accumulated_short -= shares_amount;
            }
        }
    }

    pub const fn get_accumulated_amount(&self, execution_type: ExecutionType) -> Decimal {
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
fn shares_from_amount_floor(amount: Decimal) -> u64 {
    if amount < Decimal::ZERO {
        0 // Negative amounts result in 0 shares
    } else {
        amount.floor().to_u64().unwrap_or(0) // Conservative: return 0 if conversion fails
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_calculator_new() {
        let calc = PositionCalculator::new();
        assert_eq!(calc.net_position, Decimal::ZERO);
        assert_eq!(calc.accumulated_long, Decimal::ZERO);
        assert_eq!(calc.accumulated_short, Decimal::ZERO);
    }

    #[test]
    fn test_should_execute_below_threshold() {
        let calc = PositionCalculator::with_positions(
            Decimal::ZERO,
            Decimal::new(5, 1),
            Decimal::new(8, 1),
        ); // 0.0, 0.5, 0.8
        assert!(!calc.should_execute_long());
        assert!(!calc.should_execute_short());
        assert!(calc.determine_execution_type().is_none());
    }

    #[test]
    fn test_should_execute_above_threshold() {
        let calc = PositionCalculator::with_positions(
            Decimal::ZERO,
            Decimal::new(15, 1),
            Decimal::from(2),
        ); // 0.0, 1.5, 2.0
        assert!(calc.should_execute_long());
        assert!(calc.should_execute_short());
        assert_eq!(calc.determine_execution_type(), Some(ExecutionType::Long));
    }

    #[test]
    fn test_add_trade_long_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(Decimal::new(15, 1), ExecutionType::Long); // 1.5: Buy on Schwab = accumulate for long execution
        assert_eq!(calc.accumulated_long, Decimal::new(15, 1));
        assert_eq!(calc.accumulated_short, Decimal::ZERO);
        assert_eq!(calc.net_position, Decimal::new(15, 1));
    }

    #[test]
    fn test_add_trade_short_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(Decimal::from(2), ExecutionType::Short); // 2.0: Sell on Schwab = accumulate for short execution
        assert_eq!(calc.accumulated_long, Decimal::ZERO);
        assert_eq!(calc.accumulated_short, Decimal::from(2));
        assert_eq!(calc.net_position, Decimal::from(-2));
    }

    #[test]
    fn test_add_trade_zero_amount() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(Decimal::ZERO, ExecutionType::Long); // Zero amount but still affects direction
        assert_eq!(calc.accumulated_long, Decimal::ZERO);
        assert_eq!(calc.accumulated_short, Decimal::ZERO);
        assert_eq!(calc.net_position, Decimal::ZERO);
    }

    #[test]
    fn test_add_trade_mixed_directions() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(Decimal::new(15, 1), ExecutionType::Long); // 1.5: Long accumulation
        calc.add_trade(Decimal::from(2), ExecutionType::Short); // 2.0: Short accumulation
        calc.add_trade(Decimal::new(3, 1), ExecutionType::Long); // 0.3: More long accumulation

        assert_eq!(calc.accumulated_long, Decimal::new(18, 1)); // 1.8: 1.5 + 0.3
        assert_eq!(calc.accumulated_short, Decimal::from(2)); // 2.0
        assert_eq!(calc.net_position, Decimal::new(-2, 1)); // -0.2: 1.5 - 2.0 + 0.3 = -0.2
    }

    #[test]
    fn test_reduce_accumulation() {
        let mut calc = PositionCalculator::with_positions(
            Decimal::ZERO,
            Decimal::new(25, 1),
            Decimal::from(3),
        ); // 0.0, 2.5, 3.0
        calc.reduce_accumulation(ExecutionType::Long, 2);
        assert_eq!(calc.accumulated_long, Decimal::new(5, 1)); // 0.5

        calc.reduce_accumulation(ExecutionType::Short, 1);
        assert_eq!(calc.accumulated_short, Decimal::from(2)); // 2.0
    }

    #[test]
    fn test_calculate_executable_shares() {
        let calc = PositionCalculator::with_positions(
            Decimal::ZERO,
            Decimal::new(27, 1),
            Decimal::new(32, 1),
        ); // 0.0, 2.7, 3.2
        assert_eq!(calc.calculate_executable_shares(ExecutionType::Long), 2);
        assert_eq!(calc.calculate_executable_shares(ExecutionType::Short), 3);
    }
}
