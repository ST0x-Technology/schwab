# Backfilling Improvements TODOs

## Task 1. Progress Reporting

- [x] Add optional logging/progress indicators for long-running backfill operations
- [x] Show progress like "Processing blocks 1000-2000 of 5000" during large block range queries

## Task 2. Batching with Constants

- [x] Add constant for batch size (e.g., `const BACKFILL_BATCH_SIZE: usize = 1000`) in @src/trade/mod.rs
- [x] Process block ranges in batches to prevent memory exhaustion in @src/trade/mod.rs:backfill_events
- [x] Extend test coverage to check batching

## Task 3. Error Handling with Exponential Backoff

- [x] Add `backon` crate dependency for retry mechanisms in @Cargo.toml
- [x] Create `BackfillError` enum for better error categorization (RPC failures, parsing errors, etc.) in @src/trade/backfill.rs
- [x] Implement exponential backoff for RPC failures during batch requests in @src/trade/backfill.rs:backfill_events
- [x] Consider creating a dedicated `trade::backfill` module to organize this functionality
- [x] Update @TODOs.md to reference the correct module(s) in the remaining tasks

## Task 4. Integration with Trade Batching Architecture

- [x] Move @src/trade/backfill.rs to @src/onchain/backfill.rs and update imports from `trade::` to `onchain::` types
- [x] Update backfill.rs to work with `OnchainTrade` instead of `PartialArbTrade`
- [x] Update backfill processing to use `accumulator::add_trade()` instead of direct database insertion
- [x] Ensure backfilled trades participate in the accumulation/batching system like live trades
- [x] Test that backfilled fractional trades correctly accumulate and trigger SchwabExecutions when thresholds are met
- [x] Add backfill module to @src/onchain/mod.rs exports
- [x] Remove obsolete @src/trade/ folder and update any remaining imports from `trade::` to `onchain::`

## Task 5. Code Quality Improvements

- [ ] Extract magic numbers as named constants (batch sizes, retry attempts, concurrent limits, etc.) in @src/onchain/backfill.rs and @src/lib.rs
- [ ] Improve type safety with more specific error types for better error handling granularity in @src/onchain/backfill.rs
- [ ] Make error propagation more explicit and typed

## Task 6. Enhanced Testing

- [ ] Add integration tests with realistic block ranges and data volumes in @src/onchain/backfill.rs
- [ ] Test boundary cases like deployment block equals current block in @src/onchain/backfill.rs
- [ ] Test scenarios with mixed event types and large datasets in @src/onchain/backfill.rs
- [ ] Verify proper error handling, retry mechanisms, and batching logic in @src/onchain/backfill.rs

## Task 7. SQLite-Based Queue Persistence for Idempotency

- [ ] Create `event_queue` table in SQLite schema: `id`, `tx_hash`, `log_index`, `block_number`, `event_data` (JSON), `processed` (boolean), `created_at`
- [ ] Add `(tx_hash, log_index)` unique constraint to prevent duplicate events
- [ ] Implement `enqueue_event()` function that saves events to database before processing
- [ ] Implement `get_next_unprocessed_event()` function that reads oldest unprocessed event
- [ ] Implement `mark_event_processed()` function that updates processed flag
- [ ] Update backfill logic to enqueue all discovered events instead of processing directly
- [ ] Update live event processing to enqueue events before processing
- [ ] Add startup logic that processes any unprocessed events from previous runs
- [ ] Test idempotency invariant: bot restart at any point should resume without missing/duplicating events
- [ ] Test edge cases: restart during backfill, restart during live processing, restart with empty queue

## Task 8. Queue Integration with Subscription-First Coordination

- [ ] Start WebSocket subscription immediately at application startup, buffer events in `Vec<(Event, Log)>` in @src/lib.rs:run
- [ ] Replace `tokio::sync::mpsc::unbounded_channel` with SQLite queue persistence in @src/lib.rs
- [ ] Wait for first subscription event with timeout (30s), use its `block_number` as backfill cutoff in @src/lib.rs:run
- [ ] If timeout expires with no events, fall back to `provider.get_block_number()` as cutoff in @src/lib.rs:run
- [ ] Run backfill from `deployment_block` to `cutoff_block - 1` using @src/onchain/backfill.rs:backfill_events and persist all events to queue
- [ ] Process all queued events chronologically (backfilled first, then buffered subscription events) from database in @src/lib.rs
- [ ] Continue processing live subscription events by persisting to queue then processing in @src/lib.rs:step

## Task 9. Enhanced Block Coordination and Error Handling

- [ ] Add `subscription_event_buffer: Vec<(Event, Log)>` to accumulate events during backfill phase in @src/lib.rs
- [ ] Implement backfill timeout handling: if no subscription events arrive in 30s, use current block in @src/lib.rs:run
- [ ] Add buffer size monitoring with warnings if buffer grows beyond expected limits during backfill in @src/lib.rs
- [ ] Handle subscription reconnection during backfill: restart coordination process if connection drops in @src/lib.rs
- [ ] Use database `(tx_hash, log_index)` constraint as final safety net for any edge case duplicates in @src/onchain/trade.rs:save_within_transaction
- [ ] Add comprehensive logging for coordination phases: "Subscription started", "First event at block X", "Backfill complete", "Processing buffered events" in @src/lib.rs

## Focus

The focus is on making the backfilling robust for production use with large historical datasets while keeping configuration simple through constants. The subscription-first coordination approach guarantees zero block gaps by ensuring the WebSocket subscription captures all events from block N forward while backfill handles everything up to block N-1.
