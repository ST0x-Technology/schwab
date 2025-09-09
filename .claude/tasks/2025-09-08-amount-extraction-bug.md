# 2025-09-08 Fix Amount Extraction Bug

## Problem Summary

The bot is using USDC amounts as share amounts, causing 175x overexecution. For
example:

- Onchain trade: 0.374 NVDAs1 sold for 64.17 USDC
- Bot interpreted: 64.17 shares (using USDC amount instead of NVDAs1 amount)

## Root Cause Analysis

### Primary Bug

In `src/onchain/trade.rs` lines 158-164:

```rust
// Current buggy code - only checks for "0x" suffix, missing "s1"
let (equity_amount, usdc_amount) = if onchain_output_symbol.ends_with("0x") {
    (onchain_output_amount, onchain_input_amount)
} else {
    // Falls through here for "s1" tokens, reversing the amounts!
    (onchain_input_amount, onchain_output_amount)
};
```

### Architectural Issues

1. **Duplicated suffix logic** - Different parts of code check suffixes
   differently
2. **No validation** - Silently processes invalid symbol pairs
3. **Separated validation and extraction** - Direction is determined in one
   place, amounts in another
4. **No type safety** - Can mix up USDC and share amounts

## Design Decisions

### Use Type System to Prevent Bugs

Instead of runtime validation that can be forgotten, use types that make invalid
states unrepresentable.

### Symbol Type Classification

```rust
pub enum Symbol {
    Usdc,
    TokenizedEquity { ticker: String, suffix: String },
}

impl Symbol {
    pub fn classify(symbol: &str) -> Result<Self, TradeValidationError> {
        if symbol == "USDC" {
            Ok(Symbol::Usdc)
        } else if let Some(ticker) = symbol.strip_suffix("0x") {
            Ok(Symbol::TokenizedEquity {
                ticker: ticker.to_string(),
                suffix: "0x".to_string(),
            })
        } else if let Some(ticker) = symbol.strip_suffix("s1") {
            Ok(Symbol::TokenizedEquity {
                ticker: ticker.to_string(),
                suffix: "s1".to_string(),
            })
        } else {
            Err(TradeValidationError::UnrecognizedSymbol(symbol.to_string()))
        }
    }
}
```

### Newtype Wrappers for Type Safety

```rust
#[derive(Debug, Clone, Copy)]
pub struct Shares(f64);  // Private inner value!

impl Shares {
    pub fn new(value: f64) -> Result<Self, TradeValidationError> {
        if value < 0.0 {
            return Err(TradeValidationError::NegativeShares(value));
        }
        if value > 1_000_000.0 {
            return Err(TradeValidationError::UnrealisticShareAmount(value));
        }
        Ok(Shares(value))
    }
    
    pub fn value(&self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Usdc(f64);  // Private inner value!

impl Usdc {
    pub fn new(value: f64) -> Result<Self, TradeValidationError> {
        if value < 0.0 {
            return Err(TradeValidationError::NegativeUsdc(value));
        }
        if value > 100_000_000.0 {  // $100M sanity check
            return Err(TradeValidationError::UnrealisticUsdcAmount(value));
        }
        Ok(Usdc(value))
    }
    
    pub fn value(&self) -> f64 {
        self.0
    }
}
```

## Implementation Plan

## Task 1. Create centralized symbol pair processing function

- [x] Create centralized function that processes symbol pairs and amounts
- [x] Support both "0x" and "s1" suffixes (fixing the missing "s1" support that
      caused the bug)
- [x] Reuse existing `determine_schwab_trade_details()` for validation and
      direction
- [x] Extract correct equity vs USDC amounts based on symbol pair analysis
- [x] Add comprehensive tests including the key bug fix test case
- [x] Keep solution simple without over-engineering

### Completed Changes

- Added `process_symbol_pair_and_amounts()` function in `src/onchain/trade.rs`
- Centralized logic that was previously scattered and buggy, now fixed
- Handles both "0x" and "s1" suffixes correctly (the missing piece causing the
  175x bug)
- Returns `(ticker, equity_amount, usdc_amount, direction)` tuple
- Leverages existing validation via `determine_schwab_trade_details()`
- Added focused tests including the critical test case showing 0.374 NVDAs1 vs
  64.17 USDC amounts

## Task 2. Create newtype wrappers for Shares and Usdc

- [x] Create Shares struct with private f64 field
- [x] Implement Shares::new with validation (non-negative)
- [x] Implement Shares::value() getter method
- [x] Create Usdc struct with private f64 field
- [x] Implement Usdc::new with validation (non-negative)
- [x] Implement Usdc::value() getter method
- [x] Add validation error types for invalid amounts
- [x] Add unit tests for valid construction
- [x] Add unit tests for validation failures

### Completed Changes

- Added `Shares` newtype with private f64 field and validated construction
- Added `Usdc` newtype with private f64 field and validated construction
- Both types validate only legitimate business rules:
  - Non-negative amounts only (no arbitrary upper bounds)
- Added error variants to `TradeValidationError`: `NegativeShares`,
  `NegativeUsdc`
- Added comprehensive tests covering:
  - Valid construction and value retrieval
  - Negative amount validation failures
  - Equality comparison testing
- Both types are `pub(crate)` to maintain minimal visibility

## Task 3. Fix amount extraction in try_from_order_and_fill_details

- [x] Replace buggy amount extraction logic (lines 240-246) with centralized
      TradeDetails::try_from_io
- [x] Use the existing centralized helper to ensure all symbol
      pair/amount/direction extraction uses the same logic
- [x] Preserve original tokenized equity symbol with its suffix (0x or s1)
- [x] Ensure no diverging behavior can occur by using single source of truth
- [x] Fixed the missing "s1" suffix support that caused the 175x overexecution
      bug

### Completed Changes

- Replaced the buggy `if onchain_output_symbol.ends_with("0x")` logic that only
  checked "0x" suffix
- Now calls `TradeDetails::try_from_io()` which correctly handles both "0x" and
  "s1" suffixes
- Preserves the original tokenized equity symbol from onchain data instead of
  hardcoding format
- Eliminated the separate `determine_schwab_trade_details` call since
  TradeDetails handles it internally
- All tests passing, confirming the fix works correctly
- **BONUS: Updated TradeDetails to use Shares and Usdc newtypes for additional
  type safety**

## Task 4. Remove duplicated suffix checking logic

- [x] Remove duplicated `is_tokenized_equity` closures from
      `TradeDetails::try_from_io` and `determine_schwab_trade_details`
- [x] Create centralized `is_tokenized_equity_symbol()` function
- [x] Create centralized `extract_ticker_from_symbol()` function
- [x] Update `extract_ticker_from_0x_symbol` to use centralized logic
- [x] Update CLI code (`src/cli.rs`) to use centralized suffix stripping
- [x] Update `trade_execution_link.rs` to use centralized suffix stripping
- [x] Update `accumulator.rs` to use centralized suffix stripping
- [x] Ensure all suffix handling goes through centralized functions

### Completed Changes

- Added two centralized functions in `src/onchain/trade.rs`:
  - `is_tokenized_equity_symbol(symbol: &str) -> bool` - checks for "0x" or "s1"
    suffixes
  - `extract_ticker_from_symbol(symbol: &str) -> Option<String>` - extracts
    ticker by removing suffix
- Replaced all duplicated suffix checking logic across the codebase:
  - `TradeDetails::try_from_io` - removed local `is_tokenized_equity` closure
  - `determine_schwab_trade_details` - removed local `is_tokenized_equity`
    closure
  - `extract_ticker_from_0x_symbol` - now uses centralized
    `extract_ticker_from_symbol`
  - `src/cli.rs` - replaced `.strip_suffix("0x")` with centralized function
  - `src/onchain/trade_execution_link.rs` - replaced `.strip_suffix("0x")` with
    centralized function
  - `src/onchain/accumulator.rs` - replaced manual suffix stripping with
    centralized function
- All suffix handling now goes through the same centralized logic, preventing
  future diverging behavior
- Fixed clippy warnings for optimal functional programming patterns
- All tests passing (377 tests) and code passes static analysis

## Task 5. Complete type-safe symbol architecture

- [ ] Fix create_trade_execution_linkages query to use LIKE pattern matching
      instead of hardcoded suffixes
- [ ] Update create_execution_within_transaction to take `symbol: &EquitySymbol`
      instead of `&str`
- [ ] Update SchwabExecution struct from `symbol: String` to
      `symbol: EquitySymbol`
- [ ] Update lock.rs functions to use `&EquitySymbol` instead of `&str`
- [ ] Fix remaining accumulator.rs references to use EquitySymbol properly
- [ ] Ensure all database operations use to_string() for saving and parse() for
      loading
- [ ] Test that trades with different suffixes (GME0x, GMEs1) accumulate
      together correctly

### Design Rationale

The architecture should maintain clear separation:

- **onchain_trades**: Stores TokenizedEquitySymbol (what was actually traded
  on-chain)
- **trade_accumulators**: Stores EquitySymbol (base symbols for aggregation)
- **schwab_executions**: Stores EquitySymbol (base symbols for Schwab trading)
- **symbol_locks**: Uses EquitySymbol (locks per underlying asset)

This ensures we aggregate all trades for the same underlying asset regardless of
their tokenized suffix, while preserving the actual on-chain trade information.

## Task 6. Add comprehensive tests

- [ ] Create test for TX 0x844...a42d4 (should extract 0.374 NVDAs1, not
      64.169234)
- [ ] Create test for TX 0x700...bfb85 (should extract 0.2 NVDAs1, not
      34.645024)
- [ ] Create test for GME trades with 0x suffix
- [ ] Test both USDC error case
- [ ] Test both tokenized error case
- [ ] Test unrecognized symbol error case
- [ ] Test negative amount validation
- [ ] Test unrealistic amount validation
- [ ] Add integration test with real ClearV2 event data
- [ ] Test that original suffix is preserved in output

## Task 7. Update existing code to use new types

- [ ] Update code to use Shares::value() for accessing share amounts
- [ ] Update code to use Usdc::value() for accessing USDC amounts
- [ ] Update all callers of try_from_order_and_fill_details
- [ ] Update database serialization to use .value() methods
- [ ] Ensure Schwab execution code uses .value() for amounts
- [ ] Update logging to use .value() for display
- [ ] Run full test suite to catch any breakage

## Testing Strategy

### Unit Tests

1. Symbol classification for all valid patterns
2. Symbol classification errors for invalid inputs
3. Trade extraction with NVDAs1 (s1 suffix)
4. Trade extraction with GME0x (0x suffix)
5. Error handling for invalid symbol pairs
6. Newtype validation (negative amounts, unrealistic amounts)

### Integration Tests

Use real transaction data to verify:

- Correct amount extraction
- Correct direction determination
- Original suffix preservation

### Test Data

From actual failed transactions:

- NVDA trades: 0.374, 0.2, 0.2, 0.2, 0.238 shares (not 64, 35, 35, 35, 42)
- GME trades: 0.2 shares each (not 5.2, 5.1, etc.)

## Benefits

1. **Type safety** - Can't mix up USDC and share amounts at compile time
2. **Validated construction** - Can't create invalid amounts
3. **Single source of truth** - Symbol classification in one place
4. **Forced validation** - Can't create trades without proper classification
5. **No silent failures** - Explicit errors for invalid symbols
6. **Preserves information** - Keeps original suffix from onchain data

## Task 7. Add Dry-Run Mode for Safe Testing

- [ ] Add `--dry-run` CLI flag to Env struct (default: false)
- [ ] Modify `execute_schwab_order` function to check dry-run flag before
      placing orders
- [ ] Update `Order.place` method to accept dry-run flag parameter
- [ ] Handle database updates appropriately in dry-run mode (still track
      accumulator, use special status)
- [ ] Add comprehensive logging with "[DRY-RUN]" prefixes for all simulated
      actions
- [ ] Test dry-run mode to ensure no actual Schwab API calls are made

### Design Goals

- Allow full bot testing without financial risk
- Process all onchain events normally
- Log detailed information about what would be executed
- Update database tracking appropriately
- Maintain same code paths except for actual API calls

### Implementation Strategy

1. **Environment Configuration**: Add `dry_run: bool` field to `Env` struct with
   CLI flag support
2. **Order Execution Mock**: When dry-run enabled, log order details and return
   mock success response
3. **Database Handling**: Continue accumulator tracking but mark executions with
   dry-run status
4. **Clear Logging**: Prefix all dry-run actions with "[DRY-RUN]" for visibility
5. **API Call Prevention**: Ensure no actual HTTP requests to Schwab API during
   dry-run

This enables safe testing of the amount extraction bug fix and general bot
functionality without risking real trades.

## Risk Mitigation

- All changes are compile-time checked by Rust's type system
- Comprehensive test coverage with real transaction data
- No panics - all validation returns proper Result types
- Clear error messages for debugging
- Newtype validation prevents invalid values from entering the system
