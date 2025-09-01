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

- **Enhanced proactive cleanup**: Modified `try_acquire_execution_lease()` to
  clean up ALL stale locks (not just for the current symbol), ensuring any stale
  locks get cleaned when any trade processing occurs
- **Added comprehensive logging**: Added info/warn logs for lock acquisition,
  cleanup, and clearing to improve observability
- **Added test coverage**: Added `test_cleanup_multiple_stale_locks()` to verify
  that acquiring a lease for one symbol cleans up stale locks for all symbols
- **Manual fix applied**: Cleared the stale GME lock from August 29th,
  immediately unblocking the accumulated trades

**Critical Fix**: The arbitrage bot can now recover from stale locks
automatically without manual intervention. When any new trade arrives for any
symbol, it will clean up stale locks for all symbols, ensuring no symbol remains
permanently blocked.

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

- [ ] Add `check_all_accumulated_positions` function to accumulator.rs
- [ ] Call it after processing unprocessed events in conductor.rs
- [ ] Add post-commit check after each trade in process_queued_event
- [ ] Add test coverage for accumulated position execution
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`
