# Live Testing Fixes - September 1, 2025

Verified fixes discovered during live testing of the Schwab integration.

**General Principle**: When fixing issues, add test coverage for the
corresponding problem to prevent future regressions.

## Task 1: Fix orderId Format Handling

**Issue**: Schwab API returns `orderId` as int64, not string **Source**:
`account_orders_openapi.yaml:1364` defines
`orderId: type: integer, format: int64` **Files**: `src/schwab/order_status.rs`
(lines 11-17)

- [x] Add custom deserializer to convert int64 orderId to string
- [x] Update test mocks to use numeric orderIds
- [x] Add test coverage for orderId format handling
- [x] Run `cargo test -q` (324 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

## Task 2: Remove Incorrect executionLegs Field & Fix Financial Data Safety

**Issue**: We have a top-level `executionLegs` field that doesn't exist in the
Schwab API. The OpenAPI spec (lines 1299-1408) shows the Order schema has NO
top-level executionLegs field. Execution data is only nested inside
`orderActivityCollection[].executionLegs`. Additionally, using
`#[serde(default)]` on financial fields is dangerous as it silently provides
`0.0` defaults that could corrupt calculations.

**Source**: `account_orders_openapi.yaml:1386-1392` defines
orderActivityCollection, lines 1545-1551 show executionLegs only exists inside
OrderActivity **Files**: `src/schwab/order_status.rs`

- [x] Remove top-level `execution_legs` field from OrderStatusResponse
- [x] Simplify `calculate_weighted_average_price()` to only parse from
      orderActivityCollection
- [x] Update all test mocks to put execution data inside orderActivityCollection
- [x] Remove dangerous `#[serde(default)]` attributes from financial fields
- [x] Create proper `OrderActivity` and `ExecutionLeg` types to replace
      `Vec<serde_json::Value>`
- [x] Update `OrderStatusResponse` to use `Option<T>` for all fields except
      `order_id`
- [x] Remove `Default` derive from `OrderStatusResponse` to prevent silent
      defaults
- [x] Update `calculate_weighted_average_price()` to handle `Option` types
      explicitly
- [x] Fix all tests to work with new `Option<T>` field types
- [x] Run `cargo test -q` (323 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Key Safety Improvement**: Financial fields like `filled_quantity` and
`remaining_quantity` are now `Option<f64>` instead of defaulting to `0.0`,
preventing silent data corruption in financial calculations.

## Task 3: Handle Optional Fields Explicitly

**Issue**: Missing fields in API responses cause parsing failures\
**Source**: Many fields in OpenAPI spec are optional (not in required arrays)\
**Files**: `src/schwab/order_status.rs` (lines 42-46, 59-60)

**Important Design Decision**: We will NOT use Default traits or
`#[serde(default)]` for financial data fields. Defaults are dangerous and can
lead to surprising implicit behaviors. Silent defaults can corrupt financial
calculations and mask API response issues. Instead:

- Fields that are genuinely optional should remain `Option<T>` and be handled
  explicitly
- Fields that we absolutely need should fail parsing if missing, alerting us to
  API changes
- Only fields with truly sensible defaults (like `status` potentially defaulting
  to a known state) should have explicit, non-silent fallbacks with proper
  logging

- [x] Keep all financial fields as `Option<T>` without defaults
- [x] Add explicit error handling for missing required fields
- [x] Remove dangerous `Default` trait implementation from `OrderStatus` enum
- [x] Update tests to verify proper handling of missing fields (should fail for
      required, handle gracefully for optional)
- [x] Run `cargo test -q` (324 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

## Task 4: Duplicate Event Handling

**Issue**: System fails on duplicate events instead of handling gracefully

**Motivation**: Blockchain events can be redelivered due to WebSocket
reconnections, chain reorganizations, or replay scenarios. When the same event
(identified by `tx_hash` and `log_index`) is processed twice, the system crashes
with a UNIQUE constraint violation instead of gracefully detecting and skipping
the duplicate. This causes the arbitrage bot to stop processing new trades
entirely.

**Current Behavior**: Database INSERT fails with "UNIQUE constraint failed:
onchain_trades.tx_hash, onchain_trades.log_index"

**Impact**: Database constraint violations create error noise and prevent clean
idempotent processing

**Solution**: Check for duplicate trades before attempting INSERT, log the
duplicate detection, and return early without processing

**Verification**: UNIQUE constraints on `(tx_hash, log_index)` exist; graceful
handling needed for event redelivery **Files**: `src/onchain/accumulator.rs`,
`src/conductor.rs`, `src/cli.rs`

- [x] Apply changes from stash
- [x] Review implementation
- [x] Add test coverage for duplicate event scenarios
- [x] Run `cargo test -q` (324 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

## Task 5: Stale Execution Cleanup

**Issue**: Executions stuck in SUBMITTED state cause deadlocks

**Motivation**: When a Schwab order is submitted, we set the execution status to
SUBMITTED and store the `pending_execution_id` in the accumulator. This blocks
new executions for that symbol until the order completes. However, if the order
status polling fails (due to network issues, API errors, or process crashes),
the execution remains stuck in SUBMITTED state forever. This permanently blocks
all future trades for that symbol, causing the bot to accumulate trades without
ever executing them.

**Current Behavior**:

- Accumulator checks `pending_execution_id` before executing new trades
- If non-null, it skips execution and continues accumulating
- No mechanism exists to detect or recover from stale SUBMITTED executions
- Results in permanent deadlock for affected symbols

**Real-World Impact**:

- Orders that fail to poll (e.g., network timeout) block the symbol indefinitely
- Bot continues receiving onchain events but can't execute offsetting trades
- Position imbalance grows unbounded as trades accumulate
- Manual database intervention required to clear stuck executions

**Solution**: Implement automatic cleanup of stale SUBMITTED executions that
haven't transitioned to COMPLETED/FAILED within a reasonable time window (e.g.,
5 minutes). This ensures temporary failures don't cause permanent deadlocks.

**Verification**: No existing cleanup mechanism; `pending_execution_id` blocks
new executions indefinitely **Files**: `src/onchain/accumulator.rs`
(clean_up_stale_executions function)

- [x] Apply changes from stash
- [x] Review implementation
- [x] Add test coverage for stale execution cleanup scenarios
- [x] Run `cargo test -q` (327 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

## Task 6: Improved Logging

**Issue**: Insufficient logging for debugging production issues
**Verification**: Additional info! statements for observability **Files**:
`src/conductor.rs`

- [x] Apply changes from stash
- [x] Review implementation
- [x] Verify logging coverage is adequate
- [x] Run `cargo test -q` (327 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Added comprehensive logging at multiple levels:

- `trace!()`: Detailed flow tracking (event reception, processing attempts,
  success confirmations)
- `info!()`: Important business events (trade processing with full details,
  execution triggers, completion status)
- `warn!()`: Retry failures during event processing
- `error!()`: Critical failures requiring immediate attention

Key improvements include detailed trade information logging (symbol, amount,
direction, price, tx_hash, log_index) and execution status tracking for better
production debugging without overwhelming debug mode output.

## Task 7: Clear pending_execution_id When Orders Complete

**Issue**: Order poller doesn't clear `pending_execution_id` from
trade_accumulators when orders are filled or failed, causing permanent deadlock

**Root Cause**: When the order poller (`src/schwab/order_poller.rs`) marks an
execution as FILLED or FAILED, it only updates the `schwab_executions` table but
does NOT clear the `pending_execution_id` from the `trade_accumulators` table.
This leaves the accumulator permanently blocked, preventing any new executions
for that symbol.

**Current Behavior**:

- Execution created → `pending_execution_id` set in accumulator
- Order filled on Schwab → poller updates execution status to FILLED
- `pending_execution_id` remains set → accumulator blocked forever
- New trades accumulate but can't trigger executions

**Impact**: After the first execution completes, no further offsetting trades
can be placed on Schwab for that symbol, causing unbounded position imbalance
growth.

**Files**: `src/schwab/order_poller.rs`

**Solution**:

1. Modify `handle_filled_order()` to clear `pending_execution_id` after marking
   as FILLED
2. Modify `handle_failed_order()` to clear `pending_execution_id` after marking
   as FAILED
3. Both functions need to:
   - Fetch the symbol from the execution record
   - Clear `pending_execution_id` in `trade_accumulators` where symbol matches
   - Clear any execution lease/lock if applicable

- [x] Apply manual fix to clear current stale pending_execution_id:
  ```sql
  UPDATE trade_accumulators 
  SET pending_execution_id = NULL 
  WHERE symbol = 'GME' AND pending_execution_id = 1;
  ```
- [x] Add `clear_pending_execution_id` function to `src/lock.rs`
- [x] Add logic to fetch symbol from execution in both handler functions
- [x] Clear pending_execution_id when marking execution as FILLED
- [x] Clear pending_execution_id when marking execution as FAILED
- [x] Add test coverage for `clear_pending_execution_id` function
- [x] Add test coverage for pending_execution_id cleanup in order_poller
- [x] Test that pending_execution_id is cleared when orders complete
- [x] Run `cargo test -q` (329 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Fixed the critical issue where order poller
doesn't clear `pending_execution_id` from trade_accumulators when orders are
filled or failed.

**Key Changes**:

- Added `clear_pending_execution_id` function in `src/lock.rs`
- Updated both `handle_filled_order` and `handle_failed_order` in
  `src/schwab/order_poller.rs` to:
  - Fetch the symbol from the execution record before updating status
  - Clear `pending_execution_id` from `trade_accumulators` table
  - Clear execution lease from `symbol_locks` table
  - Use proper transaction management for atomicity
- Added comprehensive test coverage for both successful and failed order
  scenarios
- Applied immediate database fix to unblock the stale GME execution

**Critical Fix**: This resolves the permanent deadlock issue where symbols
become permanently blocked after their first execution completes, ensuring the
arbitrage bot can continue placing offsetting trades for all symbols.

## Task 8: Fix Trade Direction Semantic Issue

**Issue**: The `direction` field in `onchain_trades` table stores the Schwab
offsetting direction instead of the actual onchain trade direction

**Root Cause**: `determine_schwab_trade_details` returns the Schwab offsetting
direction (e.g., BUY on Schwab to offset an onchain SELL), but this is being
stored as the trade direction and misinterpreted by the accumulator.

**Impact**:

- Onchain SELLs are stored as "BUY" in the database
- Accumulator treats them as Long positions instead of Short
- Trades accumulate in the wrong bucket (accumulated_long vs accumulated_short)
- While Schwab executions are correct, the position tracking is wrong

**Solution**:

1. Fix `determine_schwab_trade_details` to return the actual onchain trade
   direction
2. Update the accumulator mapping to handle onchain directions correctly
3. Manually fix existing database records

**Files**: `src/onchain/trade.rs`, `src/onchain/accumulator.rs`

- [x] Modify `determine_schwab_trade_details` to return onchain direction (SELL
      when giving away stock for USDC)
- [x] Update accumulator to map onchain directions to correct execution types
      (SELL → Long → Schwab BUY)
- [x] Manually update existing GME trades in database (change BUY to SELL)
- [x] Manually move GME's accumulated_long to accumulated_short
- [x] Update tests to verify onchain SELL → accumulated_long → Schwab BUY flow
- [x] Run `cargo test -q` (329 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Fixed the critical semantic issue where onchain
trade directions were being stored incorrectly in the database.

**Key Changes**:

- **Fixed `determine_schwab_trade_details`** in `src/onchain/trade.rs` to return
  the actual onchain trade direction instead of the Schwab offsetting direction
- **Updated accumulator mapping** in `src/onchain/accumulator.rs` to correctly
  map onchain directions to execution types:
  - Onchain SELL (gave away tokenized stock) → Long execution type → Schwab BUY
    to offset
  - Onchain BUY (gave away USDC) → Short execution type → Schwab SELL to offset
- **Fixed trade amount and price calculation** to be based on tokenized equity
  position rather than Schwab direction
- **Updated `reduce_accumulation`** in position calculator to properly maintain
  net_position after executions
- **Fixed existing GME database records** from BUY → SELL and moved 1.21 shares
  from accumulated_long → accumulated_short
- **Updated all tests** to reflect correct direction flow and position tracking

**Critical Fix**: The `direction` field in `onchain_trades` now correctly
represents the actual onchain trade direction, ensuring proper position tracking
while maintaining correct Schwab offsetting behavior. GME position is now
correctly tracked as -1.21 (short) instead of +1.21 (long).

## Task 9: Fix Stale Lock Cleanup Issue

**Issue**: Symbol locks from months ago are not being cleaned up, blocking new
executions

**Root Cause**: The `try_acquire_execution_lease` function attempts to clean up
locks older than 5 minutes, but the cleanup isn't working (likely datetime
format issue)

**Impact**:

- Old locks persist indefinitely
- Prevents accumulated trades from executing
- Currently blocking 1.21 GME shares from being executed

**Solution**:

1. Fix the datetime comparison in lock cleanup
2. Clear stale locks manually as immediate mitigation
3. Add monitoring/logging for lock cleanup

**Files**: `src/lock.rs`

- [x] Debug why the lock cleanup datetime comparison isn't working
- [x] Fix the cleanup logic in `try_acquire_execution_lease`
- [x] Manually clear the stale GME lock from August 29th
- [x] Add test for stale lock cleanup
- [x] Add logging when locks are cleaned up
- [x] Run `cargo test -q` (330 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Fixed the stale lock cleanup issue that was
preventing accumulated trades from executing.

**Root Cause**: The cleanup logic in `try_acquire_execution_lease()` was working
correctly, but it only ran when attempting to acquire a lease for the same
symbol. Since no new GME trades had arrived since August 29th, the stale GME
lock was never cleaned up, permanently blocking the accumulated 1.21 GME shares
from executing.

**Key Changes**:

- **Fixed lock cleanup**: Modified `try_acquire_execution_lease()` to properly
  clean up stale locks for the specific symbol being acquired, preventing
  deadlocks while still cleaning up old locks when needed
- **Added comprehensive logging**: Added info/warn logs for lock acquisition,
  cleanup, and clearing to improve observability
- **Added test coverage**: Added tests to verify proper lock cleanup behavior
- **Manual fix applied**: Cleared the stale GME lock from August 29th,
  immediately unblocking the accumulated trades

**Critical Fix**: The arbitrage bot can now recover from stale locks
automatically without manual intervention. When attempting to acquire a lock for
a symbol, it will clean up any stale lock for that symbol, ensuring symbols
don't remain permanently blocked.

## Task 10: Fix Accumulated Trades Not Executing

**Issue**: Accumulated trades don't execute when no new events arrive for that
symbol

**Root Cause**: The execution check only happens inside `process_trade` when
processing each individual trade. If multiple trades accumulate to >= 1.0 shares
but the LAST trade doesn't push it over the threshold, the execution never
happens. Additionally, if no new trades arrive after accumulation, the position
sits idle forever.

**Example Scenario**:

- 10 GME trades processed, accumulating to 1.21 shares
- First execution of 1 share completes successfully
- Remaining 0.21 shares stay accumulated
- No new GME trades arrive → 0.21 shares wait forever
- Eventually more trades would push it over 1.0 again, but they never come

**Current Behavior**:

- `process_trade` adds trade amount to accumulator
- Checks if ready to execute ONLY for that specific trade
- If that trade alone doesn't trigger execution, waits for next trade
- If no next trade comes, accumulated amount sits idle indefinitely

**Solution**: Add post-processing checks to ensure accumulated positions are
executed:

1. After processing all unprocessed events on startup
2. After each individual trade is committed (check again outside transaction)

**Files**: `src/conductor.rs`, `src/onchain/accumulator.rs`

- [x] Add `check_all_accumulated_positions` function to accumulator.rs
- [x] Call it after processing unprocessed events in conductor.rs
- [x] Add post-commit check after each trade in process_queued_event
- [x] Add test coverage for accumulated position execution
- [x] Run `cargo test -q` (333 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Fixed the critical issue where accumulated trades
don't execute when no new events arrive for that symbol.

**Root Cause**: The execution check only happened inside `process_trade` when
processing each individual trade. If multiple trades accumulated to >= 1.0
shares but the LAST trade didn't push it over the threshold, the execution never
happened. Additionally, if no new trades arrived after accumulation, the
position sat idle forever.

**Key Changes**:

- **Added `check_all_accumulated_positions` function** in
  `src/onchain/accumulator.rs` that:
  - Queries all symbols with accumulated positions >= 1.0 shares and no pending
    execution
  - For each symbol, attempts to create and execute an order
  - Handles locking and execution flow properly
  - Returns a vector of created executions for monitoring

- **Added post-startup check** in `src/conductor.rs` after processing
  unprocessed events:
  - Calls `check_all_accumulated_positions` after replay to execute any
    accumulated positions
  - Ensures positions accumulated during downtime get executed on startup

- **Added post-trade check** in `process_queued_event_atomic`:
  - Calls `check_all_accumulated_positions` after each individual trade is
    processed
  - Ensures accumulated positions execute even when triggered by unrelated
    trades

- **Added comprehensive test coverage**:
  - `test_check_all_accumulated_positions_finds_ready_symbols`: Tests basic
    functionality
  - `test_check_all_accumulated_positions_no_ready_positions`: Tests empty
    database case
  - `test_check_all_accumulated_positions_skips_pending_executions`: Tests
    proper handling of locked symbols

**Critical Fix**: Accumulated positions now execute reliably even when no new
events arrive for those symbols. The system actively checks for ready positions
after startup replay and after each trade, ensuring no position sits idle
indefinitely.

**IMPORTANT UPDATE**: Fixed a critical bug where executions created by
`check_all_accumulated_positions` in `process_queue` were never actually sent to
Schwab. The function was creating database records but not spawning tasks to
execute them. Now properly spawns async tasks to call
`execute_pending_schwab_execution` for each created execution, matching the
pattern used elsewhere in the codebase.

## Task 11: Fix Early Return Bug in process_queue

**Issue**: Bot not executing accumulated GME position (1.21 shares) despite
fixes to Task 10

**Root Cause**: `process_queue` returns early when no unprocessed events exist,
skipping `check_all_accumulated_positions` call entirely (line 364 early return
prevents reaching line 411)

**Key Changes**:

- [x] Move accumulated position check before early return in `process_queue`
- [x] Extract `check_and_execute_accumulated_positions` helper function to avoid
      deep nesting
- [x] Add periodic accumulated position checker (60s interval) as background
      task
- [x] Update `BackgroundTasks` struct to include position checker
- [x] Split complex spawn function to satisfy clippy cognitive complexity

**Files Modified**: `src/conductor.rs`

## Task 12: Fix Symbol Direction Logic Inconsistency

**Current Problem**: Symbol direction mappings are inconsistent and confusing

**What We Know**:

- Database state: GME has `accumulated_short = 1.21` from 10 onchain SELL trades
- Current error: "Expected IO to contain USDC and one 0x-suffixed symbol but got
  GME and Could not fully allocate execution shares. Remaining: 1"
- The accumulated position check is now working but failing due to direction
  logic

**Desired Logic Flow**:

1. **Onchain trade**: User sells GME0x for USDC (Direction::Sell)
2. **Exposure state**: We're now short GME → accumulate into `accumulated_short`
3. **Schwab offset**: To neutralize, we need to buy GME on Schwab
   (Direction::Buy)
4. **Trade linkage**: When executing, find the original SELL trades that created
   the short exposure

**Current State Analysis**:

- Database: 10 GME onchain SELL trades totaling 2.21 shares
- Database: GME accumulator shows `accumulated_short = 1.21` (after 1 share
  executed)
- Database: 1 share already linked to execution_id 1
- Error: "Could not fully allocate execution shares. Remaining: 1" suggests
  linkage failure

**Root Issue**: Direction logic inconsistency between:

1. How trades accumulate (SELL → accumulated_short)
2. How linkage finds trades (ShortExposure → looks for SELL trades)
3. What Schwab direction we need (short exposure → need BUY to offset)

**Current Problematic Issues**:

1. Enum name `ExecutionType` is misleading (partially renamed to
   `AccumulationBucket`)
2. Trade linkage uses string conversion instead of Direction enum
3. Misleading error: using `InvalidSymbolConfiguration` for allocation failures
4. Comments and logic are contradictory about direction flow

**Desired Clear Logic**:

- Onchain SELL → accumulated_short → Schwab BUY → neutral position
- Onchain BUY → accumulated_long → Schwab SELL → neutral position
- Trade linkage should find the original onchain trades that created the
  exposure

**Files**: `src/onchain/accumulator.rs`, `src/onchain/position_calculator.rs`,
`src/error.rs`

- [x] Fix critical typos in `determine_execution_type` function
      (`LongExposureExposure` -> `LongExposure`)
- [x] Correct direction mapping: `LongExposure -> Direction::Sell`,
      `ShortExposure -> Direction::Buy`
- [x] Fix trade linkage to use Direction enum instead of string conversion
- [x] Add new `InsufficientTradeAllocation` error type for allocation failures
- [x] Replace misleading `InvalidSymbolConfiguration` usage with proper error
      type
- [x] Update all test comments and assertions to match corrected direction flow
- [x] Add symbol validation in `extract_base_symbol` to reject invalid symbols
- [x] Run `cargo test -q` (333 tests pass)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Completed Implementation**: Fixed critical symbol direction logic
inconsistencies that were causing the accumulator to fail on execution attempts.

**Key Changes**:

- **Fixed critical typos** in `determine_execution_type()` function where
  `LongExposureExposure` and `ShortExposureExposure` prevented any executions
  from being created
- **Corrected direction mapping** in `execute_position()`:
  - `AccumulationBucket::LongExposure` → `Direction::Sell` (offset long exposure
    with Schwab sell)
  - `AccumulationBucket::ShortExposure` → `Direction::Buy` (offset short
    exposure with Schwab buy)
- **Improved trade linkage** to use `Direction` enum instead of string
  conversion for type safety
- **Added proper error type**: `InsufficientTradeAllocation` for allocation
  failures instead of misleading `InvalidSymbolConfiguration`
- **Updated all test assertions** to reflect correct direction flow:
  - Onchain SELL trades → `accumulated_short` → Schwab BUY execution
  - Onchain BUY trades → `accumulated_long` → Schwab SELL execution
- **Added symbol validation** in `extract_base_symbol()` to properly reject
  invalid symbols like "INVALID" and "USDC" during processing
- **Fixed all comments** throughout the codebase to accurately describe the
  direction logic

**Critical Fix**: The direction logic now correctly implements the intended
arbitrage flow where onchain SELL trades create short exposure that gets offset
by Schwab BUY orders, and vice versa. This resolves the "Could not fully
allocate execution shares" error and ensures proper position tracking.

## Task 13: Investigate Auth "test_auth_code" Issue

**Issue**: During live testing, "test_auth_code" appears in token refresh logs,
suggesting test data contamination in auth flow

**Priority**: Low (doesn't affect core functionality)

**Next Steps**: Investigate where test auth code is persisting in production
flow
