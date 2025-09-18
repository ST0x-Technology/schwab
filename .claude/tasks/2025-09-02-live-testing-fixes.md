# 2025-09-02 Live Testing Fixes

## Task 1: Fix Unified Trade Processing Pipeline

### Problem Summary

During live testing, we identified critical architectural and data issues:

**Logic Issues:**

- Multiple code paths for processing trades (live vs queued vs backfilled)
- `process_queued_event_atomic` tries to save trades directly, causing UNIQUE
  constraint violations
- Inconsistent duplicate handling between conductor and accumulator
- System "knows" where trades came from instead of treating them uniformly

**Data Issues Caused:**

- 18 unprocessed events in queue that are duplicates of already-processed trades
- Trades 19-22 (at 15:29:50) exist in `onchain_trades` but didn't trigger Schwab
  executions
- Trade 23 (at 15:31:04) processed live and incorrectly consumed positions from
  trades 16-19
- Error spam from duplicate constraint violations during backfill

### Implementation Checklist

- [x] Create unified event processor function
- [x] Update process_queue to use unified processor
- [x] Simplify live event handler to only enqueue
- [x] Create dedicated queue processor service
- [x] Update main lib.rs to spawn queue processor
- [x] Delete problematic functions - COMPLETED
- [x] Add comprehensive test coverage - COMPLETED (tests updated and passing)
- [x] Execute database cleanup - COMPLETED
- [x] Test the complete flow - COMPLETED (all integration tests passing)

## Task 2: ✅ COMPLETED - Unified Event Processor

### Design Principle ✅ IMPLEMENTED

All trades flow through a single processing pipeline:

```
Events (Live/Backfill) → Event Queue → Queue Processor Service → Accumulator → Schwab
```

**Key Achievement**: The system now processes trades like a fold/scan operation
over an ordered event stream, with NO separate code paths.

### Final Implementation

- [x] **Created `process_next_queued_event` function** in
      `src/conductor.rs:421-504`

  **Location**: `src/conductor.rs:421-504`\
  **Purpose**: Single event processing function used by `run_queue_processor`

- [x] **Added helper functions** - COMPLETED

  **Locations**:
  - `get_next_unprocessed_event`: `src/conductor.rs:506-520`
  - `mark_event_processed`: `src/conductor.rs:522-531`

- [x] **Integrated with BackgroundTasks** (Task 10) - COMPLETED

  **Result**: Queue processor now runs as a unified background service alongside
  token refresher, order poller, etc.

## Task 3: ~~Update Queue Processing~~ SUPERSEDED BY TASK 10

**⚠️ This task was superseded by Task 10's unified architecture.**

Originally planned to update `process_queue` function, but Task 10 implemented a
cleaner solution:

- **DELETED**: `process_queue` and `process_all_queued_events` functions
- **UNIFIED**: All event processing now handled by `run_queue_processor` service
- **SIMPLIFIED**: No separate startup vs continuous processing flows

## Task 4: Create Dedicated Queue Processor Service

### New Architecture: Separate Queue Processing Service

- [x] Create `run_queue_processor` service in `src/conductor.rs` - COMPLETED

```rust
pub(crate) async fn run_queue_processor<P: Provider + Clone>(
    env: &Env,
    pool: &SqlitePool,
    cache: &SymbolCache,
    provider: P,
) -> anyhow::Result<()> {
    info!("Starting queue processor service");
    
    loop {
        match process_next_queued_event(env, pool, cache, &provider).await {
            Ok(Some(execution)) => {
                if let Some(exec_id) = execution.id {
                    // Execute Schwab trade
                    if let Err(e) = execute_pending_schwab_execution(env, pool, exec_id).await {
                        error!("Failed to execute Schwab order {exec_id}: {e}");
                    }
                }
            }
            Ok(None) => {
                // No events to process, sleep briefly
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                error!("Error processing queued event: {e}");
                // Continue processing other events
            }
        }
    }
}
```

### Simplify Live Event Handler

- [x] Modify `process_live_event` to ONLY enqueue (no processing) - COMPLETED

```rust
async fn process_live_event(
    pool: &SqlitePool,
    event: TradeEvent,
    log: Log,
) -> Result<(), Error> {
    // Only enqueue the event - processing happens in the queue processor service
    match &event {
        TradeEvent::ClearV2(clear) => {
            trace!(
                "Enqueueing ClearV2 event: tx_hash={:?}, log_index={:?}",
                log.transaction_hash, log.log_index
            );
            enqueue(pool, clear.as_ref(), &log).await?;
        }
        TradeEvent::TakeOrderV2(take) => {
            trace!(
                "Enqueueing TakeOrderV2 event: tx_hash={:?}, log_index={:?}",
                log.transaction_hash, log.log_index
            );
            enqueue(pool, take.as_ref(), &log).await?;
        }
    }
    
    Ok(())
}
```

## Task 5: Update Main Loop to Spawn Queue Processor

### Update `src/lib.rs` to spawn queue processor service

- [x] Spawn queue processor alongside event listeners - COMPLETED

```rust
// In the run() function, after backfill completes:

// Spawn the queue processor service
let queue_processor_handle = {
    let env_clone = env.clone();
    let pool_clone = pool.clone();
    let cache_clone = symbol_cache.clone();
    let provider_clone = provider.clone();
    
    tokio::spawn(async move {
        if let Err(e) = run_queue_processor(
            &env_clone,
            &pool_clone,
            &cache_clone,
            provider_clone
        ).await {
            error!("Queue processor service failed: {e}");
        }
    })
};

// Then spawn the event listeners (existing code)
// ...

// Wait for all services
tokio::select! {
    _ = queue_processor_handle => {
        error!("Queue processor service terminated unexpectedly");
    }
    _ = clear_handle => {
        error!("Clear event listener terminated unexpectedly");
    }
    _ = take_handle => {
        error!("Take event listener terminated unexpectedly");
    }
}
```

## Task 6: Remove Problematic Functions

### Functions to Delete

- [x] Delete `process_queued_event_atomic` from `src/conductor.rs` - COMPLETED
- [x] Delete `process_queued_event_with_retry` from `src/conductor.rs` -
      COMPLETED
- [x] Delete `reprocess_unprocessed_events` from `src/conductor.rs` - COMPLETED
      (function didn't exist)
- [x] Delete obsolete test `test_process_queued_event_atomic_missing_id` -
      COMPLETED
- [x] Remove unused imports and parameters - COMPLETED

These functions caused duplicate insert attempts and have been replaced by the
unified processor.

## Task 7: ✅ COMPLETED - Test Coverage

### Current Test Status

**Verification**: `cargo test -q --lib conductor` → 16 tests passing ✅

### Unit Tests

- [x] **Duplicate Handling Tests** ✅ IMPLEMENTED
  - `test_onchain_trade_duplicate_handling` (lines 754-782)
  - `test_duplicate_trade_handling` (lines 784-817)
  - **Coverage**: Both accumulator-level and event processing duplicate handling

- [x] **Queue Processing Order Tests** ✅ IMPLEMENTED
  - `test_deterministic_processing_order` (lines 955-1025)
  - **Coverage**: Events processed in correct block_number/log_index order

- [x] **Event Processing Idempotency** ✅ IMPLEMENTED
  - `test_idempotency_bot_restart_during_processing` (lines 895-953)
  - **Coverage**: Bot restart scenarios and event replay handling

### Integration Tests

- [x] **Complete Processing Flow** ✅ IMPLEMENTED
  - `test_complete_event_processing_flow` (lines 819-893)
  - **Coverage**: Full pipeline from event → enqueue → process → accumulation

- [x] **Live Event Processing** ✅ IMPLEMENTED
  - `test_process_live_event_clear_v2` (lines 1313-1341)
  - **Coverage**: Live event enqueueing and filtering

- [x] **Edge Case Handling** ✅ IMPLEMENTED
  - `test_restart_scenarios_edge_cases` (lines 1027-1099)
  - `test_event_enqueued_when_trade_conversion_returns_none` (lines 720-752)
  - **Coverage**: Various failure and edge case scenarios

### Test Verification Command

```bash
# Run all conductor tests
cargo test -q --lib conductor

# Run full test suite  
cargo test -q
```

**Current Status**: All 331 tests passing across the codebase ✅

## Task 8: Fix Data Issues

### Database Cleanup Script

- [x] Create and execute cleanup script - COMPLETED

```sql
-- Step 1: Remove trades 19-23 (backfilled trades that didn't get proper processing)
DELETE FROM onchain_trades WHERE id >= 19;

-- Step 2: Remove execution 5 and its incorrect links
DELETE FROM trade_execution_links WHERE execution_id = 5;
DELETE FROM schwab_executions WHERE id = 5;

-- Step 3: Clean the event queue of unprocessed duplicates
DELETE FROM event_queue WHERE processed = 0;

-- Step 4: Reset accumulator to correct state after trades 1-18
UPDATE trade_accumulators
SET net_position = 0.248179435045452,
    accumulated_long = 2.92326375878456,
    accumulated_short = 2.6750843237391,
    pending_execution_id = NULL
WHERE symbol = 'GME';
```

**Cleanup Results:**

- Removed trades 19-23 (so they can be reprocessed correctly on next backfill)
- Removed execution 5 and its links (incorrect execution that consumed wrong
  positions)
- Cleared 18 duplicate events from queue
- Reset accumulator to correct state after trades 1-18

### Verification Queries

- [x] Run verification after cleanup - COMPLETED:

```sql
-- Should show 18 trades (1-18, removed 19-23)
SELECT COUNT(*) FROM onchain_trades;
-- Result: 18 ✓

-- Should show 0 unprocessed events
SELECT COUNT(*) FROM event_queue WHERE processed = 0;
-- Result: 0 ✓

-- Should show 4 executions (after removing #5)  
SELECT COUNT(*) FROM schwab_executions;
-- Result: 4 ✓

-- Should show reset accumulator state
SELECT * FROM trade_accumulators;
-- Result: GME|0.248179435045452|2.92326375878456|2.6750843237391||2025-09-02 21:00:19 ✓
```

## Task 9: Testing and Deployment

### Testing Steps

- [x] Deploy code changes to test environment - COMPLETED
- [x] Execute database cleanup script - COMPLETED
- [ ] Run backfill with fixed code to reprocess trades 19-22
- [x] Run integration tests - COMPLETED (all tests passing)
- [ ] Monitor live system logs for:
  - [ ] No UNIQUE constraint violations
  - [ ] Trades 1-18 detected as duplicates and skipped gracefully
  - [ ] Trades 19-22 processed successfully when rebackfilled
  - [ ] Appropriate Schwab executions triggered for trades 19-22
- [ ] Verify final database state matches expectations

### Success Criteria

- [ ] All historical events process before live events
- [ ] No duplicate insert errors in logs
- [ ] Trades 19-22 trigger 1-2 Schwab executions
- [ ] Accumulator shows balanced state
- [ ] System continues processing live events normally

## Key Architectural Improvements

1. **True Microservices Architecture**: Separate services for event ingestion vs
   processing
2. **Single Processing Path**: All events (live and backfilled) flow through
   identical pipeline
3. **Complete Separation of Concerns**: Event ingestion only enqueues, queue
   processor only processes
4. **Natural Backpressure**: If processing is slow, events queue up without
   blocking ingestion
5. **Service Independence**: Queue processor can be restarted without affecting
   event ingestion
6. **Clean Event Flow**: Events (Live/Backfill) → Queue → Processor Service →
   Accumulator → Schwab
7. **Maintained Ordering**: Historical events complete before live events start
   processing
8. **Proper Duplicate Handling**: Only accumulator checks/handles duplicates
9. **Resilient Architecture**: Each service can fail/restart independently
10. **Comprehensive Testing**: Unit and integration tests for all scenarios

## Implementation Summary

### ✅ COMPLETED TASKS

**Architectural Fixes:**

- ✅ Implemented unified event processor (`process_next_queued_event`)
- ✅ Created dedicated queue processor service (`run_queue_processor`)
- ✅ Simplified live event handler to only enqueue events
- ✅ Updated main loop to spawn separate queue processor service
- ✅ Removed problematic functions that caused UNIQUE constraint violations

**Code Quality:**

- ✅ Deleted `process_queued_event_atomic` and `process_queued_event_with_retry`
- ✅ Removed unused imports and parameters
- ✅ Updated all tests and confirmed they pass (331 tests total)
- ✅ **CRITICAL FIX**: Fixed silent failure handling in event processing
- ✅ **CRITICAL FIX**: Events only marked processed after successful handling
- ✅ **CRITICAL FIX**: Added proper error logging (ERROR level for failures)
- ✅ **CRITICAL FIX**: Distinguished filtered vs failed TakeOrderV2 events

**Database Cleanup:**

- ✅ Removed trades 19-23 (will be reprocessed correctly on next backfill)
- ✅ Removed incorrect execution #5 and its links
- ✅ Cleared 18 duplicate events from queue
- ✅ Reset accumulator to correct state after trades 1-18

### 🔄 READY FOR TESTING

The system is now ready to:

1. **Reprocess trades 19-22**: Next backfill will find and properly process
   these trades
2. **Handle duplicates gracefully**: No more UNIQUE constraint violations
3. **Process events in order**: Historical events complete before live events
4. **Trigger proper executions**: Trades 19-22 will get their Schwab executions

### 📋 NEXT STEPS

1. Run the backfill process with the fixed code
2. Monitor for successful processing of trades 19-22
3. Verify Schwab executions are triggered correctly
4. Confirm no duplicate constraint violations in logs

## Task 10: Unify Queue Processing and Consolidate Background Services

### Problems Identified

1. **Dual Queue Processing Flow**: We have `process_queue` (startup) and
   `run_queue_processor` (continuous), defeating the purpose of unified
   processing

2. **Scattered Background Services**: Services are spawned in different places
   making it hard to track:
   - `BackgroundTasks` in conductor.rs manages 4 services
   - Queue processor spawned separately in lib.rs
   - No unified way to manage all background services

### Solution: Complete Service Consolidation

Create a truly unified architecture where:

- ALL background services are managed through `BackgroundTasks`
- ONE queue processor handles all events (no separate startup processing)
- Clean separation between initialization and service management

### Implementation Plan

#### Step 1: Expand BackgroundTasks to Include Queue Processor ✅ COMPLETED

- [x] Add queue processor as a field in `BackgroundTasks` struct
- [x] Update `BackgroundTasks::spawn` signature to accept `SymbolCache` and
      `Provider`
- [x] Move queue processor spawning into `BackgroundTasks`
- [x] Update `wait_for_completion` to include queue processor

#### Step 2: Remove Dual Queue Processing ✅ COMPLETED

- [x] Delete `process_queue` function entirely from `src/conductor.rs`
- [x] Delete helper function `process_all_queued_events` (was already removed)
- [x] Keep only `run_queue_processor` as the single processing service
- [x] Add startup logging to `run_queue_processor` about unprocessed event count

#### Step 3: Refactor lib.rs Service Management ✅ COMPLETED

- [x] Remove separate queue processor spawning
- [x] Remove `process_queue` call after backfilling
- [x] Pass all required dependencies to `BackgroundTasks::spawn`
- [x] Use `run_live` to manage all background tasks through conductor

#### Step 4: Improve run_live Function ✅ COMPLETED

- [x] Updated `run_live` to accept cache and provider parameters
- [x] Have it spawn ALL background services via `BackgroundTasks`
- [x] Simplified the service management architecture
- [x] Better error handling with proper typed errors (removed anyhow)

### New Architecture

```rust
// In conductor.rs
pub(crate) struct BackgroundTasks {
    pub(crate) token_refresher: JoinHandle<()>,
    pub(crate) order_poller: JoinHandle<()>, 
    pub(crate) event_receiver: JoinHandle<()>,
    pub(crate) position_checker: JoinHandle<()>,
    pub(crate) queue_processor: JoinHandle<()>,  // NEW: unified with other services
}

impl BackgroundTasks {
    pub(crate) fn spawn<P: Provider + Clone>(
        env: &Env,
        pool: &SqlitePool,
        cache: &SymbolCache,
        provider: P,
        event_sender: UnboundedSender<(TradeEvent, Log)>,
        shutdown_rx: &watch::Receiver<bool>,
        clear_stream: impl Stream<...>,
        take_stream: impl Stream<...>,
    ) -> Self {
        // Spawn ALL services in one place
    }
}

// In lib.rs - much simpler
async fn run(env: Env, pool: SqlitePool) -> Result<()> {
    // ... setup ...
    
    // Backfill historical events
    backfill_events(...).await?;
    
    // Start ALL services (including queue processor)
    conductor::run_services(env, pool, clear_stream, take_stream).await
}
```

### Benefits

1. **Single source of truth**: All background services in `BackgroundTasks`
2. **Unified processing**: One queue processor for all events
3. **Better lifecycle management**: All services start/stop together
4. **Clearer architecture**: Easy to see what services are running
5. **Simpler startup**: No special handling for "historical" vs "live" events

### Implementation Checklist ✅ ALL COMPLETED

- [x] Expand `BackgroundTasks` struct with queue_processor field
- [x] Update `BackgroundTasks::spawn` to include queue processor
- [x] Delete `process_queue` and `process_all_queued_events` functions
- [x] Update `run_queue_processor` with startup logging
- [x] Simplify lib.rs to use unified service management
- [x] Update tests to reflect new architecture
- [x] Improve error handling (replaced anyhow with proper typed errors)
- [x] Update this planning document after implementation

## ✅ TASK 10 IMPLEMENTATION SUMMARY

Successfully unified the queue processing and consolidated all background
services into a clean, single-responsibility architecture:

**Key Changes Made:**

1. **Unified BackgroundTasks**: Added queue processor to `BackgroundTasks`
   struct, making it the single place where all services are managed
2. **Eliminated Dual Processing**: Removed `process_queue` function entirely -
   now only `run_queue_processor` handles all events uniformly
3. **Simplified Service Management**: Updated `run_live` to pass all
   dependencies to `BackgroundTasks::spawn`, eliminating scattered service
   spawning
4. **Proper Error Handling**: Replaced `anyhow` with typed
   `EventProcessingError` throughout conductor.rs for better error propagation
5. **Enhanced Startup Logging**: Added unprocessed event count logging to queue
   processor for better observability

**Architecture Improvements:**

- **Single Source of Truth**: All background services now managed through one
  interface
- **TRUE Unified Processing**: Every event (historical and live) processed
  identically
- **Cleaner Lifecycle Management**: Services start/stop together, easier
  debugging
- **Better Type Safety**: Proper error types instead of generic anyhow errors
- **Maintainable Code**: No more duplicate queue processing functions to keep in
  sync

**Test Results**: All 331 tests passing ✅

The system now has a clean, unified architecture where the queue processor is
properly integrated with other background services, eliminating the
architectural inconsistencies that were causing bugs.

## Notes

- The accumulator already has proper duplicate checking via
  `Trade::exists_in_db()`
- The event queue uses INSERT OR IGNORE to prevent duplicate entries
- Symbol locks ensure sequential processing per symbol
- This architecture treats all trades uniformly regardless of origin
  (live/backfill/manual)
- Historical-before-live ordering is maintained by the existing flow in
  `src/lib.rs`
