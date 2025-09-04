# 2025-09-03 Remaining Fixes

This file documents the remaining unresolved issues from the previous planning
files (2025-09-01 and 2025-09-02) that need to be addressed.

## Task 1: Complete BackgroundTasksBuilder Refactoring

### ✅ COMPLETED

**Problem Summary**

The uncommitted changes showed a fully implemented BackgroundTasksBuilder
pattern that was ready for commit. All tests were passing and the implementation
was complete.

### Implementation Checklist

- [x] Review and complete the uncommitted changes to `src/conductor.rs`
- [x] Review and complete the uncommitted changes to `src/lib.rs`
- [x] Ensure the BackgroundTasksBuilder pattern is properly implemented
- [x] Fix any compilation issues
- [x] Run tests to ensure everything passes (should be 331 tests)
- [x] Commit the completed refactoring

### Implementation Summary

The BackgroundTasksBuilder refactoring implemented:

- **Builder Pattern**: Clean dependency injection for BackgroundTasks with
  proper encapsulation
- **Unified Service Management**: All background services (token refresher,
  order poller, event receiver, position checker, queue processor) managed
  through single struct
- **Simplified Architecture**: lib.rs simplified by eliminating duplicate queue
  processing logic
- **Type Safety**: Replaced anyhow errors with proper EventProcessingError
  throughout conductor.rs
- **Enhanced Logging**: Added startup logging for unprocessed event counts

All 331 tests pass, demonstrating the refactoring is functionally complete and
maintains backward compatibility.

## Task 2: Fix Accumulator Triggering Logic (HIGH PRIORITY)

### ✅ COMPLETED

**Problem Summary**

**CRITICAL ISSUE**: The accumulator was triggering offsetting trades
incorrectly. According to the live testing notes:

> "The offsetting sell was triggered when accumulated long was above 1 instead
> of net being abs >= 1"

This was a financial logic bug that could cause incorrect trade executions.

### Root Cause Analysis

- [x] Review accumulator logic in `src/onchain/accumulator.rs`
- [x] Find where the triggering condition is implemented
- [x] Identify if it's checking `accumulated_long > 1` instead of
      `abs(net_position) >= 1`
- [x] Understand why this logic was wrong

**Root Cause Found**: The `determine_execution_type()` method in
`src/onchain/position_calculator.rs` was incorrectly checking individual
accumulated buckets (`accumulated_long >= 1` OR `accumulated_short >= 1`)
instead of the net position absolute value.

### Implementation Checklist

- [x] Locate the incorrect triggering condition in accumulator code
- [x] Fix the condition to use `abs(net_position) >= 1`
- [x] Add comprehensive test coverage for the correct behavior
- [x] Verify the fix with unit tests (all 337 tests pass)
- [x] Document the change

### Implementation Summary

**Key Changes Made**:

1. **Updated `determine_execution_type()` method** in
   `src/onchain/position_calculator.rs`:
   - Replaced bucket-based logic with net position logic
   - Now triggers only when `abs(net_position) >= 1.0`
   - Returns correct execution type based on net position sign

2. **Updated `calculate_executable_shares()` method**:
   - Removed unused `execution_type` parameter
   - Now calculates executable shares based on `abs(net_position)`

3. **Updated SQL query in `check_all_accumulated_positions()`**:
   - Changed from `(accumulated_long >= 1.0 OR accumulated_short >= 1.0)`
   - To `ABS(net_position) >= 1.0`

4. **Added comprehensive test coverage**:
   - Test net position below threshold (no trigger)
   - Test negative net position (triggers BUY)
   - Test positive net position (triggers SELL)
   - Test multiple shares execution
   - Test exact threshold boundaries
   - Test zero net position (balanced, no trigger)

5. **Removed obsolete methods**:
   - `should_execute_long()` and `should_execute_short()` (no longer needed)
   - `get_accumulated_amount()` (no longer needed with net position logic)

### Why This Fix Is Critical

This fix ensures trades only trigger when there's a meaningful net position
imbalance (>= 1 share), preventing unnecessary offsetting trades when positions
are nearly balanced. For example:

- **Before**: If `accumulated_long = 1.5` and `accumulated_short = 0.8`, would
  trigger a SELL execution even though net position is only 0.7
- **After**: Same scenario would NOT trigger because `abs(0.7) < 1.0`

The system now correctly waits until the net imbalance reaches at least 1 full
share before executing offsetting trades, which is the correct financial logic
for this arbitrage bot.

## Task 3: Complete live testing

### Problem Summary

The unified event processing system has been implemented but needs final
validation in a live environment.

### Current State (Before Re-run)

**Database State:**

- 18 trades in `onchain_trades` table (9 SELLs, 9 BUYs)
- 4 Schwab executions in `schwab_executions` table (2 BUYs, 2 SELLs)
- All executions have status 'FILLED'

**Accumulator State (GME):**

- accumulated_long: 2.92326375878456
- accumulated_short: 2.6750843237391
- net_position: 0.248179435045452 (long - short)
- pending_execution_id: NULL (no pending execution)

### Missing Trades to Backfill

5 BUY trades missing from database (trades 19-23), total amount: 2.252347404
GME0x:

1. **Trade 19**: `0x851...265c9` BUY 0.446657099090545675 GME0x
2. **Trade 20**: `0xf48...6cd29` BUY 0.452396994426829503 GME0x
3. **Trade 21**: `0x483...8261d` BUY 0.450241443680909519 GME0x
4. **Trade 22**: `0x966...c4d6c` BUY 0.447145937960352371 GME0x
5. **Trade 23**: `0x01a...c9182` BUY 0.455905963200924442 GME0x

### Expected Schwab Executions

After processing the 5 missing BUY trades:

- **New accumulated_long**: 5.175610978885044 (current + 2.252347404)
- **New net_position**: 2.500526654530304 (5.1756 - 2.6751)
- **Triggering condition**: `abs(net_position) >= 1.0` → `2.5005 >= 1.0` ✓
- **Expected execution**: 2-share SELL order on Schwab
- **Post-execution net**: 0.500526654530304 (2.5005 - 2.0)

### Final Expected State

**Database State:**

- 23 trades in `onchain_trades` table
- 5 Schwab executions in `schwab_executions` table (2 BUYs, 3 SELLs)

**Final Accumulator State (GME):**

- accumulated_long: 3.175610978885044 (after 2-share reduction)
- accumulated_short: 2.6750843237391 (unchanged)
- net_position: 0.500526654530304
- pending_execution_id: NULL

### Implementation Checklist

- [ ] Run backfill to reprocess the 5 missing BUY trades
- [ ] Monitor live system logs for:
  - [ ] No UNIQUE constraint violations
  - [ ] Trades 1-18 detected as duplicates and skipped gracefully
  - [ ] Trades 19-23 processed successfully when rebackfilled
  - [ ] Exactly 1 new Schwab SELL execution for 2 shares
- [ ] Verify final database state matches expectations
- [ ] Document results

### Success Criteria

- All historical events process before live events
- No duplicate insert errors in logs
- Exactly 1 new SELL execution for 2 shares triggered
- Final net position: ~0.50 (below 1.0 threshold)
- System continues processing live events normally

## Task 4: Debug Missing Trades #19-22 (HIGH PRIORITY)

### Problem Summary

After running the bot with backfill on 2025-09-04, the expected counter-trades
did not occur. Investigation revealed that trades #19-22 are in the event_queue
and marked as processed, but never made it to the onchain_trades table. Only
trade #24 (corresponding to expected trade #23) was successfully processed.

### Current State

**Database State:**

- 19 trades in onchain_trades (missing trades #19-23)
- Trade #24 is present (corresponds to expected trade #23)
- Net position: 0.704085 (below 1.0 threshold, no execution triggered)
- 4 historical executions from Sept 2nd (2 BUY, 2 SELL, net 0)

**Event Queue State:**

- Trades #19-22 ARE in event_queue with `processed=1`
- Transaction hashes confirmed:
  - Trade 19:
    `0x85104b7b46082a22e319526bee52b0faeaaedf6c0f63c74f3897d3254c3265c9`
  - Trade 20:
    `0xf484f57ee88675ba84edae1f9a47d205630118cfaee8c4d47fde7572a896cd29`
  - Trade 21:
    `0x4834480a7871beed22be382401c84bcc7bde834871b2103af5f86d7c7de8261d`
  - Trade 22:
    `0x966b3076daa6ae0a0beee92adb7a8eb8d13ac69628fdb0ef29e01a3ab41c4d6c`

### Investigation Findings

1. **Events exist and were processed**: All 4 missing trades are in event_queue
2. **Owner addresses match**: Alice owner in events matches configured
   ORDER_OWNER
   - Event: `0x17a0b3a25eefd6b02b2c58bf5f025da5ba172f49` (lowercase)
   - Config: `0x17a0B3A25eefD6b02b2c58bf5F025da5bA172F49` (EIP-55 checksum)
3. **Using typed Address comparison**: Both sides are
   `alloy::primitives::Address` which should make case irrelevant (20-byte array
   comparison)
4. **Filtering is occurring**: Events were processed but didn't create trades

### Implementation Checklist

- [ ] Add diagnostic logging to `src/onchain/clear.rs`:
  - [ ] Log addresses being compared (alice.owner, bob.owner, env.order_owner)
  - [ ] Log owner match results (true/false for each)
  - [ ] Log when trade is filtered due to no owner match
- [ ] Enhance logging in `src/conductor.rs`:
  - [ ] Include full tx_hash in filtering log
  - [ ] Add success logging when trade IS created
- [ ] Reset database state:
  - [ ] Reset event_queue entries for trades #19-24 to `processed=0`
  - [ ] Delete trade #24 from onchain_trades
  - [ ] Adjust accumulator state
- [ ] Re-run bot with diagnostic logging
- [ ] Analyze logs to identify exact failure point
- [ ] Implement fix based on findings
- [ ] Verify all 5 trades process correctly
- [ ] Confirm 2-share SELL execution triggers

### Diagnostic SQL Commands

```sql
-- Reset event queue for trades 19-24
UPDATE event_queue 
SET processed = 0, processed_at = NULL 
WHERE tx_hash IN (
  '0x85104b7b46082a22e319526bee52b0faeaaedf6c0f63c74f3897d3254c3265c9',
  '0xf484f57ee88675ba84edae1f9a47d205630118cfaee8c4d47fde7572a896cd29',
  '0x4834480a7871beed22be382401c84bcc7bde834871b2103af5f86d7c7de8261d',
  '0x966b3076daa6ae0a0beee92adb7a8eb8d13ac69628fdb0ef29e01a3ab41c4d6c',
  '0x01ad7e96ce23e411b1761f12b544da8eada5b89a1a0636ced52b15675e0c9182'
);

-- Delete trade #24
DELETE FROM onchain_trades WHERE id = 24;

-- Reset accumulator
UPDATE trade_accumulators 
SET accumulated_long = accumulated_long - 0.455906
WHERE symbol = 'GME';
```

### Expected Resolution

Once diagnostic logging reveals the exact failure point, we expect to find
either:

1. An unexpected issue with Address comparison
2. A different validation check failing
3. An error in the trade creation logic

The fix will allow trades #19-22 to process, increasing net position to ~2.5 and
triggering the expected 2-share SELL execution.

## Task 5: Fix Auth "test_auth_code" Issue (LOW PRIORITY)

### Problem Summary

From Task 15 in the old planning file:

- "test_auth_code" appears in token refresh logs in production
- This suggests test data contamination in the auth flow
- Auth CLI command was returning 400 bad request

### Investigation Checklist

- [ ] Search codebase for "test_auth_code" references
- [ ] Check where test auth code might be persisting in production
- [ ] Review auth flow for test data contamination
- [ ] Fix auth CLI command 400 bad request error
- [ ] Ensure proper auth flow in production

### Priority

Low priority since this doesn't appear to be affecting core trading
functionality, but should be addressed for production cleanliness.

## Implementation Order

1. **Task 1**: Complete BackgroundTasksBuilder refactoring ✅ COMPLETED
2. **Task 2**: Fix accumulator triggering logic ✅ COMPLETED
3. **Task 3**: Complete live testing (validate the fixes)
4. **Task 4**: Debug missing trades #19-22 (diagnostic logging needed)
5. **Task 5**: Fix auth issue (cleanup, low priority)

## Notes

- Tasks 1 and 2 are complete and blocking issues resolved
- Task 3 is ready for execution with detailed expectations documented
- Task 4 (backfill regression) was removed after analysis showed no regression -
  all trades in database match expected onchain trades perfectly
- The unified event processing work from 2025-09-02 is complete and working (331
  tests passing)
