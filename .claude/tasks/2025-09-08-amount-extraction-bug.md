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

## Task 5. Complete type-safe symbol architecture - NOT NEEDED

This task was determined to be unnecessary because:

- [x] create_trade_execution_linkages already uses LIKE pattern matching
      (`base_symbol%`) in accumulator.rs:276
- SchwabExecution using String for symbol is not critical since the typed
  validation happens at the parsing layer
- The core typed architecture (TokenizedEquitySymbol, EquitySymbol, Shares,
  Usdc) works effectively at the validation/conversion boundaries
- Database operations correctly use the typed system for validation and
  conversion

### Status: COMPLETE (Not Needed)

The existing implementation provides sufficient type safety where it matters
most - at the parsing and validation boundaries. The database layer using String
types is acceptable since all validation happens before data reaches the
database.

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

- [x] Create test for TX 0x844...a42d4 (should extract 0.374 NVDAs1, not
      64.169234)
- [x] Create test for TX 0x700...bfb85 (should extract 0.2 NVDAs1, not
      34.645024)
- [x] Create test for GME trades with 0x and s1 suffixes mapping to same ticker
- [x] Test both USDC error case (existing test)
- [x] Test both tokenized error case (existing test)
- [x] Test unrecognized symbol error case (existing test)
- [x] Test negative amount validation (existing test)
- [x] Test edge cases with very small and very large amounts
- [x] Test that original suffix is preserved in output (via existing tests)

### Completed Changes

- Added `test_real_transaction_0x844_nvda_s1_bug_fix()` in src/onchain/io.rs:598
  - Tests the exact transaction that caused 175x overexecution bug
  - Verifies 0.374 NVDAs1 shares extracted (not 64.169234 USDC amount)
  - Validates correct price calculation (~$171.58/share)

- Added `test_real_transaction_0x700_nvda_s1_bug_fix()` in src/onchain/io.rs:615
  - Tests second real transaction with same bug pattern
  - Verifies 0.2 NVDAs1 shares extracted (not 34.645024 USDC amount)
  - Validates correct price calculation (~$173.23/share)

- Added `test_gme_trades_with_different_suffixes_extract_same_ticker()` in
  src/onchain/io.rs:632
  - Tests that GME0x and GMEs1 both map to base symbol "GME"
  - Ensures accumulation will work correctly for trades with different suffixes

- Added `test_edge_case_validation_very_small_amounts()` and
  `test_edge_case_validation_very_large_amounts()` in src/onchain/io.rs:650,659
  - Tests boundary conditions with realistic but extreme amounts
  - Ensures the system handles edge cases gracefully

All 25 tests in the io module pass, including the 5 new integration tests.

## Task 7. Update existing code to use new types

- [x] Update code to use Shares::value() for accessing share amounts
- [x] Update code to use Usdc::value() for accessing USDC amounts
- [x] Update all callers of try_from_order_and_fill_details
- [x] Update database serialization to use .value() methods
- [x] Ensure Schwab execution code uses .value() for amounts
- [x] Update logging to use .value() for display
- [x] Run full test suite to catch any breakage

### Completed Changes

**All code correctly uses the new typed system:**

- **TradeDetails** in src/onchain/io.rs uses Shares and Usdc types internally
- **Trade struct** in src/onchain/trade.rs:157,163,182 calls
  `.equity_amount().value()` and `.usdc_amount().value()`
- **Database operations** correctly use `.value()` methods for serialization
- **All callers** of `try_from_order_and_fill_details` automatically benefit
  from the typed system since TradeDetails handles the conversion internally

**Evidence of correct usage:**

```rust
// src/onchain/trade.rs:157
if trade_details.equity_amount().value() == 0.0 {

// src/onchain/trade.rs:163  
trade_details.usdc_amount().value() / trade_details.equity_amount().value();

// src/onchain/trade.rs:182
amount: trade_details.equity_amount().value(),
```

All 381+ tests pass, confirming the typed system works correctly throughout the
codebase.

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

- [x] Add `--dry-run` CLI flag to Env struct (default: false)
- [x] Create Broker trait abstraction for order execution
- [x] Implement LogBroker for dry-run mode with mock functionality
- [x] Implement Schwab broker for real order execution
- [x] Replace execute_schwab_order function with broker.execute_order method
- [x] Update OrderStatusPoller to use broker.get_order_status
- [x] Add comprehensive logging with "[DRY-RUN]" prefixes for all simulated
      actions
- [x] Test dry-run mode to ensure no actual Schwab API calls are made
- [x] Fix LogBroker order ID generation to use numeric IDs (id.to_string(), not
      format!("DRY_{id}"))
- [x] Add Clone trait to Schwab struct
- [x] Add Clone trait to LogBroker struct
- [x] Use Arc<AtomicU64> for LogBroker's order_counter for safe cloning
- [x] Update execute_pending_schwab_execution to accept broker parameter (remove
      line 707: let broker = env.get_broker())
- [x] Update run_queue_processor to accept and pass broker parameter
- [x] Update spawn_queue_processor to accept and pass broker parameter
- [x] Update process_next_queued_event to accept and pass broker parameter if
      needed
- [x] Update check_and_execute_accumulated_positions to accept broker parameter
- [x] Update periodic_accumulated_position_check to accept broker parameter
- [x] Update spawn_position_checker to accept and pass broker parameter
- [x] Update BackgroundTasksBuilder::spawn to pass broker to
      spawn_position_checker and spawn_queue_processor
- [x] Ensure no local broker creation anywhere in the main bot flow
- [x] Ensure `cargo test -q && rainix-rs-static` passes
- [x] **COMPLETED**: Create dedicated broker module with clean organization
- [x] **COMPLETED**: Implement DynBroker type alias for cleaner function
      signatures
- [x] **COMPLETED**: Remove dangerous panic! from production code
- [x] **COMPLETED**: Fix all clippy warnings and compilation errors

### Completed Changes

- **Added Broker trait abstraction** in `src/schwab/order.rs` for order
  execution:
  - `execute_order()` method for placing orders
  - `get_order_status()` method for checking order status
  - Trait bounds: `Send + Sync + Debug` for async and testing support
- **Implemented LogBroker** for dry-run mode:
  - Sequential order ID generation starting from 1,000,000,000
  - All actions logged with "[DRY-RUN]" prefixes
  - Mock successful order fills with arbitrary prices
  - No actual HTTP requests to Schwab API
- **Implemented Schwab broker** for real order execution:
  - Wraps existing order placement and status checking logic
  - Maintains all existing retry and error handling behavior
- **Updated conductor.rs and cli.rs** to use broker instances:
  - Main bot flow creates broker once and reuses it consistently
  - CLI flow creates broker locally per operation (appropriate scope)
- **Removed execute_schwab_order wrapper function**:
  - Functionality moved to Broker trait methods
  - Cleaner architecture with proper encapsulation
- **Updated OrderStatusPoller** to accept broker parameter:
  - Ensures same broker type used for execution and status checking
  - Prevents divergence between different broker implementations
- **Added dry_run field to Env struct**:
  - CLI flag support: `--dry-run` (defaults to false)
  - Factory method `env.get_broker()` returns appropriate broker type
- **Comprehensive testing**:
  - All 381 tests pass
  - No compiler warnings
  - Proper visibility levels maintained

### Architecture Benefits

1. **Type-safe broker abstraction**: Supports both real and mock brokers through
   single interface
2. **Single broker per bot run**: Prevents divergence between execution and
   status checking
3. **Clean separation of concerns**: Order logic encapsulated in broker
   implementations
4. **Future extensibility**: Easy to add other broker types (e.g., other
   brokerages)
5. **Safe testing**: Dry-run mode prevents accidental real trades during
   development
6. **Single broker instance throughout main bot flow**: All async tasks share
   the same broker instance, preventing any divergence in broker behavior
7. **Clone support for broker distribution**: Broker implementations support
   Clone for safe distribution across spawned tasks

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

## FINAL STATUS: COMPLETE ✅

### Summary

**The 175x overexecution bug has been completely fixed and all planned work is
complete.**

**Core Bug Fix:**

- ✅ Fixed missing "s1" suffix support that caused 64.17 USDC to be treated as
  shares instead of 0.374 NVDAs1
- ✅ Implemented centralized TradeDetails::try_from_io() that correctly handles
  both "0x" and "s1" suffixes
- ✅ Added type-safe Shares and Usdc newtypes to prevent future amount mix-ups
  at compile time

**Architecture Improvements:**

- ✅ Centralized all suffix checking logic to prevent future diverging behavior
- ✅ Added comprehensive type safety through TokenizedEquitySymbol and
  EquitySymbol types
- ✅ Implemented dry-run mode with Broker trait abstraction for safe testing

**Testing Coverage:**

- ✅ Added 5 new integration tests including real transaction data that caused
  the bug
- ✅ Tests prove the fix: 0.374 NVDAs1 shares extracted (not 64.17 USDC amount)
- ✅ All 381+ tests pass with the new implementation

**Benefits Achieved:**

1. **Type safety** - Can't mix up USDC and share amounts at compile time
2. **Validated construction** - Can't create invalid amounts
3. **Single source of truth** - Symbol classification in one place
4. **No silent failures** - Explicit errors for invalid symbols
5. **Preserves information** - Keeps original suffix from onchain data
6. **Safe testing** - Dry-run mode prevents accidental real trades

The bot will no longer execute 175x overexecution due to suffix parsing bugs.

## Risk Mitigation

- All changes are compile-time checked by Rust's type system
- Comprehensive test coverage with real transaction data
- No panics - all validation returns proper Result types
- Clear error messages for debugging
- Newtype validation prevents invalid values from entering the system
