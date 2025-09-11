# Stop Background Tasks on Market Close

## Current Behavior

When the stock market closes, the bot's timeout expires and drops the
`run_bot()` future, but background tasks spawned via `tokio::spawn` continue
running independently. These tasks include the queue processor which has an
infinite loop that continuously polls the database.

## Issue

- Background tasks are spawned with `tokio::spawn` which makes them **detached**
  from parent
- Dropping the parent future doesn't stop spawned tasks
- Queue processor runs infinite loop with no market hours awareness
- No mechanism to stop background tasks when market closes
- This may be causing high CPU usage during market close hours

## Solution: Abort Background Tasks on Market Close

Use the JoinHandle abort mechanism to explicitly stop all background tasks when
market closes, then restart them when market reopens.

## Implementation Plan

## 1. Make conductor::run_live Return BackgroundTasks

- [x] Change return type from `Result<()>` to `Result<BackgroundTasks>`
- [x] Return the `background_tasks` struct instead of waiting for completion
- [x] Remove the wait_for_completion call from run_live
- [x] Move event processing loop to a spawned task

**COMPLETED**: Updated in `src/conductor.rs:311-359`

**Location**: `src/conductor.rs:311-355`

## 2. Store and Abort BackgroundTasks on Market Close

- [x] In `lib.rs`, store the returned BackgroundTasks
- [x] When market close timeout expires, abort all tasks before continuing
- [x] Add proper cleanup logging
- [x] Update log messages to reflect market state changes

**COMPLETED**: Updated in `src/lib.rs:158-178`

**Location**: `src/lib.rs:158-164`

## 3. Implement Abort Method for BackgroundTasks

- [x] Add `abort_all()` method to BackgroundTasks struct
- [x] Call `.abort()` on each JoinHandle
- [x] Log task abortion

**COMPLETED**: Added method in `src/conductor.rs:206-226`

**Location**: `src/conductor.rs:85-91` and `src/conductor.rs:177-206`

## Expected Behavior After Implementation

1. Market closes → timeout expires
2. All background tasks are explicitly aborted
3. Queue processor and other background services stop immediately
4. Bot waits for market to reopen
5. New tasks are spawned when market reopens

---

## Implementation Completed Successfully

**All tasks completed on 2025-09-10:**

1. ✅ Modified `conductor::run_live` to return `BackgroundTasks` instead of
   waiting for completion
2. ✅ Updated `lib.rs` to store BackgroundTasks handle and abort tasks on market
   close timeout
3. ✅ Implemented `abort_all()` method that calls `.abort()` on each JoinHandle
4. ✅ Removed unused shutdown signal channels (they were for Ctrl+C, not market
   hours)
5. ✅ Fixed naming inconsistency (event_receiver → event_processor)
6. ✅ Verified all tests pass

**Files modified:**

- `src/conductor.rs:311-368` - Modified run_live to return BackgroundTasks
- `src/conductor.rs:197-215` - Added abort_all() method
- `src/conductor.rs:80-86` - Updated BackgroundTasks struct
- `src/lib.rs:158-178` - Updated market close handling to abort tasks
- Various spawn functions - Removed unused shutdown signal parameters

**Key Changes:**

- Background tasks are now explicitly aborted when market closes instead of
  running indefinitely
- Each market session gets fresh background tasks (no state carried over)
- Backfilling runs every time market opens (as required for 24/7 onchain market)
- Clean separation between market hours lifecycle and process termination
  (Ctrl+C)

**Testing:**

- `cargo test -q --lib` - All 391 tests pass (5 new tests added)
- No compilation errors
- **New test coverage added:**
  - `test_background_tasks_abort_all()` - Tests the abort_all method
    functionality
  - `test_background_tasks_individual_abort()` - Tests individual JoinHandle
    abort behavior
  - `test_run_live_returns_background_tasks_immediately()` - Tests that run_live
    returns quickly
  - `test_market_close_timeout_simulation()` - Simulates market close timeout
    scenario
  - `test_background_tasks_wait_vs_abort_race()` - Tests race condition between
    wait and abort
- Ready for deployment to test CPU usage behavior during market close

## Benefits of This Approach

- **Clean lifecycle management**: Clear start/stop for each market session
- **Resource efficiency**: No background tasks running during market close
- **Simple implementation**: Minimal code changes required
- **Explicit control**: Tasks are deliberately stopped rather than abandoned
- **Reliable restart**: Fresh tasks for each market session

## Alternative Approaches Considered

1. **Shutdown signals**: Only work for process termination, not market hours
   transitions
2. **Market-aware polling in each task**: Would still consume resources checking
   time
3. **Scoped tasks without spawn**: Would require major architectural changes

## Testing

- Verify all background tasks stop when market closes
- Verify tasks restart properly when market reopens
- Check logs for proper abort messages
- Monitor resource usage during market close hours
