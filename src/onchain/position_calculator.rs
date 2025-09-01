const SCHWAB_MINIMUM_WHOLE_SHARES: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccumulationBucket {
    LongExposure,
    ShortExposure,
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

    pub fn determine_execution_type(&self) -> Option<AccumulationBucket> {
        if self.should_execute_long() {
            Some(AccumulationBucket::LongExposure)
        } else if self.should_execute_short() {
            Some(AccumulationBucket::ShortExposure)
        } else {
            None
        }
    }

    pub fn add_trade(&mut self, amount: f64, direction: AccumulationBucket) {
        match direction {
            AccumulationBucket::LongExposure => {
                // Long exposure from onchain BUY -> accumulate for Schwab SELL to offset
                self.accumulated_long += amount;
                self.net_position += amount;
            }
            AccumulationBucket::ShortExposure => {
                // Short exposure from onchain SELL -> accumulate for Schwab BUY to offset
                self.accumulated_short += amount;
                self.net_position -= amount;
            }
        }
    }

    pub fn reduce_accumulation(&mut self, execution_type: AccumulationBucket, shares: u64) {
        // precision loss only occurs beyond 2^53 shares (unrealistic for equity trading)
        #[allow(clippy::cast_precision_loss)]
        let shares_amount = shares as f64;

        match execution_type {
            AccumulationBucket::LongExposure => {
                self.accumulated_long -= shares_amount;
                // AccumulationBucket::LongExposure means we executed a Schwab SELL to offset accumulated long positions
                // This reduces our net long exposure
                self.net_position -= shares_amount;
            }
            AccumulationBucket::ShortExposure => {
                self.accumulated_short -= shares_amount;
                // AccumulationBucket::ShortExposure means we executed a Schwab BUY to offset accumulated short positions
                // This reduces our net short exposure (increases net_position)
                self.net_position += shares_amount;
            }
        }
    }

    pub const fn get_accumulated_amount(&self, execution_type: AccumulationBucket) -> f64 {
        match execution_type {
            AccumulationBucket::LongExposure => self.accumulated_long,
            AccumulationBucket::ShortExposure => self.accumulated_short,
        }
    }

    pub fn calculate_executable_shares(&self, execution_type: AccumulationBucket) -> u64 {
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
        assert_eq!(
            calc.determine_execution_type(),
            Some(AccumulationBucket::LongExposure)
        );
    }

    #[test]
    fn test_add_trade_long_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(1.5, AccumulationBucket::LongExposure); // Long exposure from onchain BUY -> accumulate for Schwab SELL
        assert!((calc.accumulated_long - 1.5).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 0.0).abs() < f64::EPSILON);
        assert!((calc.net_position - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_short_accumulation() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(2.0, AccumulationBucket::ShortExposure); // Short exposure from onchain SELL -> accumulate for Schwab BUY
        assert!((calc.accumulated_long - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON);
        assert!((calc.net_position - (-2.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_zero_amount() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(0.0, AccumulationBucket::LongExposure); // Zero amount but still affects direction
        assert!((calc.accumulated_long - 0.0).abs() < f64::EPSILON);
        assert!((calc.accumulated_short - 0.0).abs() < f64::EPSILON);
        assert!((calc.net_position - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_add_trade_mixed_directions() {
        let mut calc = PositionCalculator::new();
        calc.add_trade(1.5, AccumulationBucket::LongExposure); // Long accumulation
        calc.add_trade(2.0, AccumulationBucket::ShortExposure); // Short accumulation
        calc.add_trade(0.3, AccumulationBucket::LongExposure); // More long accumulation

        assert!((calc.accumulated_long - 1.8).abs() < f64::EPSILON); // 1.5 + 0.3
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON); // 2.0
        assert!((calc.net_position - (-0.2)).abs() < f64::EPSILON); // 1.5 - 2.0 + 0.3 = -0.2
    }

    #[test]
    fn test_reduce_accumulation() {
        let mut calc = PositionCalculator::with_positions(0.0, 2.5, 3.0);
        calc.reduce_accumulation(AccumulationBucket::LongExposure, 2);
        assert!((calc.accumulated_long - 0.5).abs() < f64::EPSILON);
        assert!((calc.net_position - (-2.0)).abs() < f64::EPSILON); // 0.0 - 2.0

        calc.reduce_accumulation(AccumulationBucket::ShortExposure, 1);
        assert!((calc.accumulated_short - 2.0).abs() < f64::EPSILON);
        assert!((calc.net_position - (-1.0)).abs() < f64::EPSILON); // -2.0 + 1.0
    }

    #[test]
    fn test_calculate_executable_shares() {
        let calc = PositionCalculator::with_positions(0.0, 2.7, 3.2);
        assert_eq!(
            calc.calculate_executable_shares(AccumulationBucket::LongExposure),
            2
        );
        assert_eq!(
            calc.calculate_executable_shares(AccumulationBucket::ShortExposure),
            3
        );
    }
}
