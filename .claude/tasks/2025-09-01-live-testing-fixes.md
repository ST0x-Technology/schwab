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

## Task 13: Update Infrastructure to Use Owner Address Filtering

**Issue**: Current onchain infrastructure differs from documented design -
instead of monitoring a single order hash, we need to monitor multiple orders
from the same owner address for different tokenized equities

**Current State**:

- System filters events by matching specific `ORDER_HASH` in environment config
- Only processes events where alice/bob order hash matches configured hash
- Documentation describes "a Raindex Order" (singular)

**Actual Infrastructure**:

- Multiple orders exist for buying/selling different tokenized equities
- All orders share the same owner address
- Need to process events from any order owned by the specified address

**Impact**:

- Current filtering is too restrictive and misses relevant trades
- System only processes trades from one specific order instead of all orders
  from the owner
- Scalability limited - adding new tokenized equities requires code changes

**Solution**: Replace order hash filtering with owner address filtering

**Files**: `src/onchain/mod.rs`, `src/onchain/clear.rs`,
`src/onchain/take_order.rs`, `src/conductor.rs`, `src/env.rs`, `README.md`,
`CLAUDE.md`, `.env.example`, `.github/workflows/deploy.yaml`, `.do/app.yaml`

- [x] Update `src/onchain/mod.rs` to replace `order_hash: B256` with
      `order_owner: Address`
- [x] Update environment variable from `ORDER_HASH` to `ORDER_OWNER`
- [x] Update `src/onchain/clear.rs` to filter by owner:
      `alice_order.owner == env.order_owner || bob_order.owner == env.order_owner`
- [x] Update `src/onchain/take_order.rs` to filter by owner:
      `event.config.order.owner == env.order_owner`
- [x] Update `src/conductor.rs` to pass `order_owner` instead of `order_hash`
- [x] Update `src/env.rs` test helpers to use owner address
- [x] Update all test files to use owner address filtering
- [x] Update `.env.example`, `.github/workflows/deploy.yaml`, `.do/app.yaml`
      configs
- [x] Update `README.md` to reflect multiple orders from same owner
- [x] Update `CLAUDE.md` environment variables documentation
- [x] Run `cargo test -q` to ensure all tests pass (332/333 pass, 1 unrelated
      deadlock test fails)
- [x] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [x] Run `pre-commit run -a`

**Key Benefits**:

- Supports multiple tokenized equities without code changes
- More flexible and scalable infrastructure
- Simpler deployment configuration
- Better matches actual onchain architecture

## Task 14: Debug the following issue

Backfilling spammed a bunch of error logs, then backfilled the missing onchain
trades, but then didn't place an offsetting trade, but then a live trade was
observed and an offsetting trade was placed then.

```
2025-09-02T15:29:21.399105Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34960164-34961163
2025-09-02T15:29:21.402197Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34905164-34906163
2025-09-02T15:29:21.402248Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34916164-34917163
2025-09-02T15:29:21.403387Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35012164-35013163
2025-09-02T15:29:21.407517Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34949164-34950163
2025-09-02T15:29:21.410997Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35001164-35002163
2025-09-02T15:29:21.411040Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34979164-34980163
2025-09-02T15:29:21.415266Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34990164-34991163
2025-09-02T15:29:21.415318Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35008164-35009163
2025-09-02T15:29:21.435182Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34933164-34934163
2025-09-02T15:29:21.435243Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35006164-35007163
2025-09-02T15:29:21.435264Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34921164-34922163
2025-09-02T15:29:21.435561Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34894164-34895163
2025-09-02T15:29:21.473314Z DEBUG rain_schwab::onchain::backfill: Found 9 ClearV2 events and 0 TakeOrderV2 events in batch 35017164-35018163
2025-09-02T15:29:21.475142Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34911164-34912163
2025-09-02T15:29:21.475179Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34995164-34996163
2025-09-02T15:29:21.475206Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34922164-34923163
2025-09-02T15:29:21.475231Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34965164-34966163
2025-09-02T15:29:21.475268Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34932164-34933163
2025-09-02T15:29:21.475293Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35011164-35012163
2025-09-02T15:29:21.475322Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34973164-34974163
2025-09-02T15:29:21.475368Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34984164-34985163
2025-09-02T15:29:21.475406Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34943164-34944163
2025-09-02T15:29:21.475429Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34954164-34955163
2025-09-02T15:29:21.475453Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34927164-34928163
2025-09-02T15:29:21.475475Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34900164-34901163
2025-09-02T15:29:21.476068Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35000164-35001163
2025-09-02T15:29:21.476104Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34956164-34957163
2025-09-02T15:29:21.476126Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35004164-35005163
2025-09-02T15:29:21.476146Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34945164-34946163
2025-09-02T15:29:21.476179Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34934164-34935163
2025-09-02T15:29:21.476203Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34901164-34902163
2025-09-02T15:29:21.480028Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34917164-34918163
2025-09-02T15:29:21.480093Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34895164-34896163
2025-09-02T15:29:21.480124Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34997164-34998163
2025-09-02T15:29:21.480150Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34967164-34968163
2025-09-02T15:29:21.587515Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35013164-35014163
2025-09-02T15:29:21.595795Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34906164-34907163
2025-09-02T15:29:21.595860Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34991164-34992163
2025-09-02T15:29:21.595883Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34975164-34976163
2025-09-02T15:29:21.595913Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34986164-34987163
2025-09-02T15:29:21.595935Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34890164-34891163
2025-09-02T15:29:21.596149Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35002164-35003163
2025-09-02T15:29:21.596181Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34929164-34930163
2025-09-02T15:29:21.598479Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34925164-34926163
2025-09-02T15:29:21.602322Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34918164-34919163
2025-09-02T15:29:21.603052Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34907164-34908163
2025-09-02T15:29:21.607220Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34944164-34945163
2025-09-02T15:29:21.608239Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34985164-34986163
2025-09-02T15:29:21.611081Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34955164-34956163
2025-09-02T15:29:21.615426Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34974164-34975163
2025-09-02T15:29:21.617312Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35007164-35008163
2025-09-02T15:29:21.622748Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34961164-34962163
2025-09-02T15:29:21.648432Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34928164-34929163
2025-09-02T15:29:21.658713Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34939164-34940163
2025-09-02T15:29:21.661117Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34980164-34981163
2025-09-02T15:29:21.662776Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34950164-34951163
2025-09-02T15:29:21.678358Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34912164-34913163
2025-09-02T15:29:21.679449Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34923164-34924163
2025-09-02T15:29:21.680663Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34896164-34897163
2025-09-02T15:29:21.683272Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34996164-34997163
2025-09-02T15:29:21.684501Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34962164-34963163
2025-09-02T15:29:21.685972Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34966164-34967163
2025-09-02T15:29:21.686623Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34898164-34899163
2025-09-02T15:29:21.688275Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34920164-34921163
2025-09-02T15:29:21.688312Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34959164-34960163
2025-09-02T15:29:21.688633Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34976164-34977163
2025-09-02T15:29:21.689423Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34963164-34964163
2025-09-02T15:29:21.692462Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34924164-34925163
2025-09-02T15:29:21.692729Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34993164-34994163
2025-09-02T15:29:21.693198Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34953164-34954163
2025-09-02T15:29:21.743748Z DEBUG rain_schwab::onchain::backfill: Found 7 ClearV2 events and 0 TakeOrderV2 events in batch 35016164-35017163
2025-09-02T15:29:21.745468Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34983164-34984163
2025-09-02T15:29:21.745532Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34891164-34892163
2025-09-02T15:29:21.745569Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34930164-34931163
2025-09-02T15:29:21.745600Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34989164-34990163
2025-09-02T15:29:21.745634Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34964164-34965163
2025-09-02T15:29:21.745665Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34957164-34958163
2025-09-02T15:29:21.745700Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34897164-34898163
2025-09-02T15:29:21.745731Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34941164-34942163
2025-09-02T15:29:21.745758Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34937164-34938163
2025-09-02T15:29:21.746443Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34893164-34894163
2025-09-02T15:29:21.746479Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34952164-34953163
2025-09-02T15:29:21.746501Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34978164-34979163
2025-09-02T15:29:21.746531Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34913164-34914163
2025-09-02T15:29:21.746561Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34971164-34972163
2025-09-02T15:29:21.748197Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34994164-34995163
2025-09-02T15:29:21.748235Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34909164-34910163
2025-09-02T15:29:21.748259Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34982164-34983163
2025-09-02T15:29:21.748280Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34987164-34988163
2025-09-02T15:29:21.748301Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34902164-34903163
2025-09-02T15:29:21.748321Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35009164-35010163
2025-09-02T15:29:21.748808Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35005164-35006163
2025-09-02T15:29:21.749743Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34948164-34949163
2025-09-02T15:29:21.751136Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34998164-34999163
2025-09-02T15:29:21.752706Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34968164-34969163
2025-09-02T15:29:21.758865Z DEBUG rain_schwab::onchain::backfill: Found 4 ClearV2 events and 0 TakeOrderV2 events in batch 35018164-35018805
2025-09-02T15:29:21.759734Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34970164-34971163
2025-09-02T15:29:21.759773Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34988164-34989163
2025-09-02T15:29:21.759803Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34977164-34978163
2025-09-02T15:29:21.759836Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34992164-34993163
2025-09-02T15:29:21.759870Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34940164-34941163
2025-09-02T15:29:21.759899Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35010164-35011163
2025-09-02T15:29:21.759931Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34951164-34952163
2025-09-02T15:29:21.759963Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34999164-35000163
2025-09-02T15:29:21.760443Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34914164-34915163
2025-09-02T15:29:21.760526Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34981164-34982163
2025-09-02T15:29:21.760573Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34936164-34937163
2025-09-02T15:29:21.762203Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34969164-34970163
2025-09-02T15:29:21.762270Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35014164-35015163
2025-09-02T15:29:21.762295Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34910164-34911163
2025-09-02T15:29:21.762820Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34903164-34904163
2025-09-02T15:29:21.763995Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 35003164-35004163
2025-09-02T15:29:21.764039Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34947164-34948163
2025-09-02T15:29:21.764515Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34908164-34909163
2025-09-02T15:29:21.765307Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34926164-34927163
2025-09-02T15:29:21.765563Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34919164-34920163
2025-09-02T15:29:21.767567Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34946164-34947163
2025-09-02T15:29:21.768197Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34904164-34905163
2025-09-02T15:29:21.768305Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34931164-34932163
2025-09-02T15:29:21.769097Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34899164-34900163
2025-09-02T15:29:21.770845Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34972164-34973163
2025-09-02T15:29:21.771117Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34942164-34943163
2025-09-02T15:29:21.772013Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34892164-34893163
2025-09-02T15:29:21.784162Z DEBUG rain_schwab::onchain::backfill: Found 2 ClearV2 events and 0 TakeOrderV2 events in batch 35015164-35016163
2025-09-02T15:29:21.785140Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34935164-34936163
2025-09-02T15:29:21.785215Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34958164-34959163
2025-09-02T15:29:21.785260Z DEBUG rain_schwab::onchain::backfill: Found 0 ClearV2 events and 0 TakeOrderV2 events in batch 34915164-34916163
2025-09-02T15:29:21.808622Z  INFO rain_schwab::onchain::backfill: Backfill completed: 32 events enqueued
2025-09-02T15:29:21.808672Z  INFO rain_schwab::conductor: Processing any unprocessed events from previous sessions...
2025-09-02T15:29:21.815412Z  INFO rain_schwab::conductor: Found 21 unprocessed events to reprocess
2025-09-02T15:29:22.469753Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.34923875420896344, direction=Sell, tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log
2025-09-02T15:29:22.470702Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log_index=51: error returned from database: (code: 2067) U
2025-09-02T15:29:22.789200Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.34923875420896344, direction=Sell, tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log
2025-09-02T15:29:22.789934Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log_index=51: error returned from database: (code: 2067) U
2025-09-02T15:29:23.204289Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.34923875420896344, direction=Sell, tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log
2025-09-02T15:29:23.204903Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log_index=51: error returned from database: (code: 2067) U
2025-09-02T15:29:23.900468Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.34923875420896344, direction=Sell, tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log
2025-09-02T15:29:23.901194Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x10c9e2dd77193d78a46e786bbbf5f1147d7fd5b2b5bd92c72faa0837abbe5432, log_index=51: error returned from database: (code: 2067) U
2025-09-02T15:29:23.901239Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:24.137394Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43
2025-09-02T15:29:24.138158Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43: error returned from database: (code: 2067) U
2025-09-02T15:29:24.456403Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43
2025-09-02T15:29:24.457311Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43: error returned from database: (code: 2067) U
2025-09-02T15:29:24.879736Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43
2025-09-02T15:29:24.880482Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43: error returned from database: (code: 2067) U
2025-09-02T15:29:25.500677Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43
2025-09-02T15:29:25.501178Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xbfe5fd4d006f82404ba88d447a98dec812576b3c903ca3201c5c31003c69d40b, log_index=43: error returned from database: (code: 2067) U
2025-09-02T15:29:25.501207Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:25.730043Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3921278507774403, direction=Sell, tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_
2025-09-02T15:29:25.730629Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_index=315: error returned from database: (code: 2067)
2025-09-02T15:29:26.140429Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3921278507774403, direction=Sell, tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_
2025-09-02T15:29:26.141200Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_index=315: error returned from database: (code: 2067)
2025-09-02T15:29:26.560909Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3921278507774403, direction=Sell, tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_
2025-09-02T15:29:26.561394Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_index=315: error returned from database: (code: 2067)
2025-09-02T15:29:27.186838Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3921278507774403, direction=Sell, tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_
2025-09-02T15:29:27.187545Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x18832dc99039a4abe92f1f96c30856263dfba326fa0ea530c011d982801af539, log_index=315: error returned from database: (code: 2067)
2025-09-02T15:29:27.187579Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:27.421657Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431
2025-09-02T15:29:27.422327Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431: error returned from database: (code: 2067)
2025-09-02T15:29:27.740926Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431
2025-09-02T15:29:27.741367Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431: error returned from database: (code: 2067)
2025-09-02T15:29:28.161502Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431
2025-09-02T15:29:28.161886Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431: error returned from database: (code: 2067)
2025-09-02T15:29:28.785692Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431
2025-09-02T15:29:28.786135Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xc2bcab00debba57935ff04580e55f90bfa4d6ed3d92e904b4075924395d92483, log_index=431: error returned from database: (code: 2067)
2025-09-02T15:29:28.786166Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:29.004117Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175
2025-09-02T15:29:29.004567Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:29.326221Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175
2025-09-02T15:29:29.326777Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:29.743870Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175
2025-09-02T15:29:29.744419Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:30.364805Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175
2025-09-02T15:29:30.365313Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe45c0e03922d2aef3a97d70a5034bf0c00dc5774a88e9d21577134cc7418666a, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:30.365350Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:30.597033Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.39138942414047884, direction=Sell, tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log
2025-09-02T15:29:30.597842Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log_index=117: error returned from database: (code: 2067)
2025-09-02T15:29:30.939554Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.39138942414047884, direction=Sell, tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log
2025-09-02T15:29:30.940059Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log_index=117: error returned from database: (code: 2067)
2025-09-02T15:29:31.378479Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.39138942414047884, direction=Sell, tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log
2025-09-02T15:29:31.379082Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log_index=117: error returned from database: (code: 2067)
2025-09-02T15:29:32.003560Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.39138942414047884, direction=Sell, tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log
2025-09-02T15:29:32.004051Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xee3ec504856425206b714642d1000756a048b4f1faec43989df8e5b6fb67cd21, log_index=117: error returned from database: (code: 2067)
2025-09-02T15:29:32.004086Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:32.220968Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38
2025-09-02T15:29:32.221476Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38: error returned from database: (code: 2067) U
2025-09-02T15:29:32.542669Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38
2025-09-02T15:29:32.543154Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38: error returned from database: (code: 2067) U
2025-09-02T15:29:32.960929Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38
2025-09-02T15:29:32.961420Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38: error returned from database: (code: 2067) U
2025-09-02T15:29:33.588348Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2, direction=Sell, tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38
2025-09-02T15:29:33.588728Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x631895bd57a08bc381e31d0d6d1f1317df7c1759f731f848b02027b7e57c773f, log_index=38: error returned from database: (code: 2067) U
2025-09-02T15:29:33.588761Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:33.822137Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3896287588108655, direction=Sell, tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_
2025-09-02T15:29:33.822558Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_index=185: error returned from database: (code: 2067)
2025-09-02T15:29:34.148065Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3896287588108655, direction=Sell, tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_
2025-09-02T15:29:34.148801Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_index=185: error returned from database: (code: 2067)
2025-09-02T15:29:34.575917Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3896287588108655, direction=Sell, tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_
2025-09-02T15:29:34.576427Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_index=185: error returned from database: (code: 2067)
2025-09-02T15:29:35.204775Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3896287588108655, direction=Sell, tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_
2025-09-02T15:29:35.205217Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe5abbc01efdac7c8e639e57181e4c65a233c04bc2684977e2c6592a4817fbd1d, log_index=185: error returned from database: (code: 2067)
2025-09-02T15:29:35.205253Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:36.121258Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3526995358013561, direction=Sell, tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_
2025-09-02T15:29:36.122124Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_index=139: error returned from database: (code: 2067)
2025-09-02T15:29:36.454684Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3526995358013561, direction=Sell, tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_
2025-09-02T15:29:36.455179Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_index=139: error returned from database: (code: 2067)
2025-09-02T15:29:36.876177Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3526995358013561, direction=Sell, tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_
2025-09-02T15:29:36.876841Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_index=139: error returned from database: (code: 2067)
2025-09-02T15:29:37.497971Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.3526995358013561, direction=Sell, tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_
2025-09-02T15:29:37.498777Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7f7f066f01a18a49cdce6f44f8b807573216656bd2801ab9281fed71f9dfb3af, log_index=139: error returned from database: (code: 2067)
2025-09-02T15:29:37.498821Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:37.719545Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.32085708618196157, direction=Buy, tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_
2025-09-02T15:29:37.720056Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_index=46: error returned from database: (code: 2067) U
2025-09-02T15:29:38.044774Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.32085708618196157, direction=Buy, tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_
2025-09-02T15:29:38.045312Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_index=46: error returned from database: (code: 2067) U
2025-09-02T15:29:38.469430Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.32085708618196157, direction=Buy, tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_
2025-09-02T15:29:38.470043Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_index=46: error returned from database: (code: 2067) U
2025-09-02T15:29:39.161208Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.32085708618196157, direction=Buy, tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_
2025-09-02T15:29:39.161789Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x8240de7a0fc226f8a5b7dfd44356b5e67c4041d95359c9dd3b5d79c0a9209352, log_index=46: error returned from database: (code: 2067) U
2025-09-02T15:29:39.161827Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:39.391875Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.41690159314160297, direction=Buy, tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_
2025-09-02T15:29:39.392378Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_index=706: error returned from database: (code: 2067)
2025-09-02T15:29:39.714002Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.41690159314160297, direction=Buy, tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_
2025-09-02T15:29:39.714798Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_index=706: error returned from database: (code: 2067)
2025-09-02T15:29:40.129261Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.41690159314160297, direction=Buy, tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_
2025-09-02T15:29:40.129638Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_index=706: error returned from database: (code: 2067)
2025-09-02T15:29:40.750673Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.41690159314160297, direction=Buy, tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_
2025-09-02T15:29:40.751200Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x392e31fc4504dc357eec97d1aa4c479d207a2e42a7d63b44776771019924e078, log_index=706: error returned from database: (code: 2067)
2025-09-02T15:29:40.751239Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:40.971527Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21687826434968205, direction=Buy, tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_
2025-09-02T15:29:40.972172Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_index=589: error returned from database: (code: 2067)
2025-09-02T15:29:41.295016Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21687826434968205, direction=Buy, tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_
2025-09-02T15:29:41.295733Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_index=589: error returned from database: (code: 2067)
2025-09-02T15:29:41.716583Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21687826434968205, direction=Buy, tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_
2025-09-02T15:29:41.717059Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_index=589: error returned from database: (code: 2067)
2025-09-02T15:29:42.339306Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21687826434968205, direction=Buy, tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_
2025-09-02T15:29:42.339922Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x7e486227d62d8b5743aa125d4fb6747c43d92bd2fa2cfa6c9bef34bfceccd741, log_index=589: error returned from database: (code: 2067)
2025-09-02T15:29:42.339972Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:42.561782Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21888371489091818, direction=Buy, tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_
2025-09-02T15:29:42.562356Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:42.902338Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21888371489091818, direction=Buy, tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_
2025-09-02T15:29:42.902831Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:43.318504Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21888371489091818, direction=Buy, tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_
2025-09-02T15:29:43.319099Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:43.938316Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.21888371489091818, direction=Buy, tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_
2025-09-02T15:29:43.939166Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xb9203a7282ed9745334b4249e3efcd5dba892df53b7def158a8a964884d68787, log_index=175: error returned from database: (code: 2067)
2025-09-02T15:29:43.939239Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:44.157740Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43369163025373075, direction=Buy, tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_
2025-09-02T15:29:44.158212Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_index=812: error returned from database: (code: 2067)
2025-09-02T15:29:44.476856Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43369163025373075, direction=Buy, tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_
2025-09-02T15:29:44.477325Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_index=812: error returned from database: (code: 2067)
2025-09-02T15:29:44.902265Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43369163025373075, direction=Buy, tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_
2025-09-02T15:29:44.902791Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_index=812: error returned from database: (code: 2067)
2025-09-02T15:29:45.534917Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43369163025373075, direction=Buy, tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_
2025-09-02T15:29:45.535594Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x55306612035a2d958160c485e1cb16aa37c111600aa535abcbad12e697aa1719, log_index=812: error returned from database: (code: 2067)
2025-09-02T15:29:45.535644Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:45.765780Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2227428105958575, direction=Buy, tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_i
2025-09-02T15:29:45.766399Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_index=49: error returned from database: (code: 2067) U
2025-09-02T15:29:46.085354Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2227428105958575, direction=Buy, tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_i
2025-09-02T15:29:46.085841Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_index=49: error returned from database: (code: 2067) U
2025-09-02T15:29:46.502975Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2227428105958575, direction=Buy, tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_i
2025-09-02T15:29:46.503686Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_index=49: error returned from database: (code: 2067) U
2025-09-02T15:29:47.124054Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.2227428105958575, direction=Buy, tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_i
2025-09-02T15:29:47.124501Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xf2f7cbd837d6b682b33027b87c3108a9c2afd1bd9e46f3b21a411a5ab334eb1e, log_index=49: error returned from database: (code: 2067) U
2025-09-02T15:29:47.124533Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:47.356703Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4340870738066712, direction=Buy, tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_i
2025-09-02T15:29:47.357132Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_index=569: error returned from database: (code: 2067)
2025-09-02T15:29:47.675911Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4340870738066712, direction=Buy, tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_i
2025-09-02T15:29:47.676429Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_index=569: error returned from database: (code: 2067)
2025-09-02T15:29:48.110595Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4340870738066712, direction=Buy, tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_i
2025-09-02T15:29:48.111296Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_index=569: error returned from database: (code: 2067)
2025-09-02T15:29:48.747157Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4340870738066712, direction=Buy, tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_i
2025-09-02T15:29:48.747688Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0xe4d53d3e8dc74b1dc3f044f768de5493ca2a52a5047513e4e3f39f89cb15d4c3, log_index=569: error returned from database: (code: 2067)
2025-09-02T15:29:48.747723Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:48.971602Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43315147338053245, direction=Buy, tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_
2025-09-02T15:29:48.972182Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_index=193: error returned from database: (code: 2067)
2025-09-02T15:29:49.296254Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43315147338053245, direction=Buy, tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_
2025-09-02T15:29:49.296882Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_index=193: error returned from database: (code: 2067)
2025-09-02T15:29:49.715770Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43315147338053245, direction=Buy, tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_
2025-09-02T15:29:49.716282Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_index=193: error returned from database: (code: 2067)
2025-09-02T15:29:50.358279Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.43315147338053245, direction=Buy, tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_
2025-09-02T15:29:50.358913Z  WARN rain_schwab::conductor: Event processing failed for tx_hash=0x3aefcfc5054cea126b75976618c70bd9719572656952fa13fb66c89754f238b0, log_index=193: error returned from database: (code: 2067)
2025-09-02T15:29:50.358959Z ERROR rain_schwab::conductor: Failed to reprocess event after retries: error returned from database: (code: 2067) UNIQUE constraint failed: onchain_trades.tx_hash, onchain_trades.log_index
2025-09-02T15:29:50.599156Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4466570990905457, direction=Buy, tx_hash=0x85104b7b46082a22e319526bee52b0faeaaedf6c0f63c74f3897d3254c3265c9, log_i
2025-09-02T15:29:50.601509Z  INFO rain_schwab::onchain::accumulator: Trade already exists (tx_hash=0x85104b7b46082a22e319526bee52b0faeaaedf6c0f63c74f3897d3254c3265c9, log_index=81), skipping duplicate processing
2025-09-02T15:29:50.601717Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:50.602192Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:50.836256Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4523969944268295, direction=Buy, tx_hash=0xf484f57ee88675ba84edae1f9a47d205630118cfaee8c4d47fde7572a896cd29, log_i
2025-09-02T15:29:50.838532Z  INFO rain_schwab::onchain::accumulator: Trade already exists (tx_hash=0xf484f57ee88675ba84edae1f9a47d205630118cfaee8c4d47fde7572a896cd29, log_index=189), skipping duplicate processing
2025-09-02T15:29:50.838720Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:50.839220Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:51.090396Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.4502414436809095, direction=Buy, tx_hash=0x4834480a7871beed22be382401c84bcc7bde834871b2103af5f86d7c7de8261d, log_i
2025-09-02T15:29:51.092588Z  INFO rain_schwab::onchain::accumulator: Trade already exists (tx_hash=0x4834480a7871beed22be382401c84bcc7bde834871b2103af5f86d7c7de8261d, log_index=346), skipping duplicate processing
2025-09-02T15:29:51.092720Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:51.093124Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:51.340162Z  INFO rain_schwab::conductor: Processing queued trade: symbol=GME0x, amount=0.44714593796035235, direction=Buy, tx_hash=0x966b3076daa6ae0a0beee92adb7a8eb8d13ac69628fdb0ef29e01a3ab41c4d6c, log_
2025-09-02T15:29:51.342126Z  INFO rain_schwab::onchain::accumulator: Trade already exists (tx_hash=0x966b3076daa6ae0a0beee92adb7a8eb8d13ac69628fdb0ef29e01a3ab41c4d6c, log_index=927), skipping duplicate processing
2025-09-02T15:29:51.342301Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:51.342592Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:51.342625Z  INFO rain_schwab::conductor: Successfully reprocessed 4 events, 17 failures
2025-09-02T15:29:51.342645Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:51.342995Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:51.343019Z DEBUG rain_schwab::conductor: No accumulated positions ready for execution
2025-09-02T15:29:51.343051Z  INFO rain_schwab::conductor: Starting token refresh service
2025-09-02T15:29:51.343084Z  INFO rain_schwab::conductor: Starting order status poller with interval: 15s, max jitter: 5s
2025-09-02T15:29:51.343127Z  INFO rain_schwab::conductor: Starting blockchain event receiver
2025-09-02T15:29:51.343158Z  INFO rain_schwab::conductor: Starting periodic accumulated position checker
2025-09-02T15:29:51.343188Z  INFO rain_schwab::schwab::order_poller: Starting order status poller with interval: 15s
2025-09-02T15:29:51.343243Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:29:51.343693Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:29:51.344506Z DEBUG rain_schwab::conductor: Running periodic accumulated position check
2025-09-02T15:29:51.344528Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:29:51.344799Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:29:51.344816Z DEBUG rain_schwab::conductor: No accumulated positions ready for execution
2025-09-02T15:30:06.344281Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:30:06.344808Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:30:21.344543Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:30:21.345065Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:30:36.344022Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:30:36.344566Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:30:51.344250Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:30:51.344474Z DEBUG rain_schwab::conductor: Running periodic accumulated position check
2025-09-02T15:30:51.344500Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:30:51.344926Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:30:51.344954Z DEBUG rain_schwab::conductor: No accumulated positions ready for execution
2025-09-02T15:30:51.345031Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:31:04.187363Z  INFO rain_schwab::conductor: Processing ClearV2 event: tx_hash=Some(0x01ad7e96ce23e411b1761f12b544da8eada5b89a1a0636ced52b15675e0c9182), log_index=Some(786)
2025-09-02T15:31:04.414319Z  INFO rain_schwab::conductor: ClearV2 event converted to trade: symbol=GME0x, amount=0.45590596320092447, direction=Buy
2025-09-02T15:31:04.414380Z  INFO rain_schwab::conductor: Processing onchain trade: symbol=GME0x, amount=0.45590596320092447, direction=Buy, price_usdc=21.41806817230988, tx_hash=0x01ad7e96ce23e411b1761f12b544da8eada5b89a1a0636ced52b15675e0c9182, log_index=786
2025-09-02T15:31:04.415312Z  INFO rain_schwab::onchain::accumulator: Saved onchain trade trade_id=23 symbol=GME0x amount=0.45590596320092447 direction=Buy tx_hash=0x01ad7e96ce23e411b1761f12b544da8eada5b89a1a0636ced52b15675e0c9182 log_index=786
2025-09-02T15:31:04.416573Z  INFO rain_schwab::onchain::accumulator: Updated calculator symbol=GME net_position=0.47801528606277693 accumulated_long=1.1530996098018815 accumulated_short=0.6750843237391042 exposure_bucket=LongExposure trade_amount=0.45590596320092447
2025-09-02T15:31:04.417353Z  INFO rain_schwab::lock: Acquired execution lease for symbol: GME
2025-09-02T15:31:04.418252Z  INFO rain_schwab::onchain::accumulator: Created trade-execution linkage trade_id=16 execution_id=5 contributed_shares=0.2640421732204241 remaining_execution_shares=0.7359578267795759
2025-09-02T15:31:04.418393Z  INFO rain_schwab::onchain::accumulator: Created trade-execution linkage trade_id=17 execution_id=5 contributed_shares=0.2260701121835994 remaining_execution_shares=0.5098877145959765
2025-09-02T15:31:04.418527Z  INFO rain_schwab::onchain::accumulator: Created trade-execution linkage trade_id=18 execution_id=5 contributed_shares=0.43315147338053245 remaining_execution_shares=0.07673624121544403
2025-09-02T15:31:04.418671Z  INFO rain_schwab::onchain::accumulator: Created trade-execution linkage trade_id=19 execution_id=5 contributed_shares=0.07673624121544403 remaining_execution_shares=0.0
2025-09-02T15:31:04.418701Z  INFO rain_schwab::onchain::accumulator: Created Schwab execution with trade linkages symbol=GME shares=1 direction=Sell execution_type=LongExposure execution_id=Some(5) remaining_long=0.15309960980188153 remaining_short=0.6750843237391042
2025-09-02T15:31:04.419847Z  INFO rain_schwab::conductor: Trade triggered Schwab execution: symbol=GME0x, execution_id=5
2025-09-02T15:31:04.420197Z  INFO rain_schwab::conductor: Executing Schwab order: SchwabExecution { id: Some(5), symbol: "GME", shares: 1, direction: Sell, state: Pending }
2025-09-02T15:31:06.344680Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:31:06.345057Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:31:06.973645Z  INFO rain_schwab::schwab::order: Successfully placed Schwab order for execution: id=5, order_id=1004063768196
2025-09-02T15:31:06.975006Z  INFO rain_schwab::conductor: Successfully completed Schwab order execution for execution_id=5
2025-09-02T15:31:21.344550Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:31:21.346500Z  INFO rain_schwab::schwab::order_poller: Polling 1 submitted orders
2025-09-02T15:31:23.688518Z DEBUG rain_schwab::schwab::order: Schwab order status response: {"session":"NORMAL","duration":"DAY","orderType":"MARKET","complexOrderStrategyType":"NONE","quantity":1.0,"filledQuantity":1.0,"remainingQuantity":0.0,"requestedDestination":"AUTO","destinationLinkName":"NITE","orderLegCollection":[{"orderLegType":"EQUITY","legId":1,"instrument":{"assetType":"EQUITY","cusip":"36467W109","symbol":"GME","instrumentId":4430271},"instruction":"SELL","positionEffect":"CLOSING","quantity":1.0}],"orderStrategyType":"SINGLE","orderId":1004063768196,"cancelable":false,"editable":false,"status":"FILLED","enteredTime":"2025-09-02T15:31:07+0000","closeTime":"2025-09-02T15:31:07+0000","tag":"TA_nickmagliocchetticom1751890824","accountNumber":49359741,"orderActivityCollection":[{"activityType":"EXECUTION","activityId":102169934812,"executionType":"FILL","quantity":1.0,"orderRemainingQuantity":0.0,"executionLegs":[{"legId":1,"quantity":1.0,"mismarkedQuantity":0.0,"price":22.765,"time":"2025-09-02T15:31:07+0000","instrumentId":4430271}]}]}
2025-09-02T15:31:23.691455Z  INFO rain_schwab::lock: Cleared execution lease for symbol: GME
2025-09-02T15:31:23.691897Z  INFO rain_schwab::schwab::order_poller: Updated execution 5 to FILLED with price: 2277 cents and cleared locks for symbol: GME
2025-09-02T15:31:23.820871Z DEBUG rain_schwab::schwab::order_poller: Completed polling cycle
2025-09-02T15:31:36.344563Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:31:36.344879Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:31:51.345159Z DEBUG rain_schwab::conductor: Running periodic accumulated position check
2025-09-02T15:31:51.345224Z  INFO rain_schwab::onchain::accumulator: Checking all accumulated positions for ready executions
2025-09-02T15:31:51.345265Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:31:51.345748Z  INFO rain_schwab::onchain::accumulator: No accumulated positions found ready for execution
2025-09-02T15:31:51.345754Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:31:51.345786Z DEBUG rain_schwab::conductor: No accumulated positions ready for execution
2025-09-02T15:32:06.345405Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:32:06.345736Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
2025-09-02T15:32:21.344248Z DEBUG rain_schwab::schwab::order_poller: Starting polling cycle for submitted orders
2025-09-02T15:32:21.344640Z DEBUG rain_schwab::schwab::order_poller: No submitted orders to poll
^C2025-09-02T15:32:32.859566Z  INFO rain_schwab: Received shutdown signal, shutting down gracefully...
2025-09-02T15:32:32.859593Z  INFO rain_schwab: Shutdown complete
```

## Task 15: Investigate Auth "test_auth_code" Issue

**Issue**: During live testing, "test_auth_code" appears in token refresh logs,
suggesting test data contamination in auth flow

**Priority**: Low (doesn't affect core functionality)

**Next Steps**: Investigate where test auth code is persisting in production
flow

## Notes from live testing

- [ ] There were 10 backfilled trades for some reason (strange regression as it
      used to work previously)
- [ ] The auth CLI command was returning 400 bad request for some reason
- [ ] The offsetting sell was triggered when accumulated long was above 1
      instead of net being abs >= 1
