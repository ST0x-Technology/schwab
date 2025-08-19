# Backfilling Improvements TODOs

The focus is on making the backfilling robust for production use while keeping configuration simple. The subscription-first coordination MUST guarantee zero block gaps by ensuring the WebSocket subscription captures all events from block N forward while backfill handles everything up to block N-1. Queue-based architecture MUST ensure idempotency across multiple runs of the bot using SQLite as the persistence layer.

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

- [x] Extract magic numbers as named constants (batch sizes, retry attempts, concurrent limits, etc.) in @src/onchain/backfill.rs and @src/lib.rs
- [x] Use itertools where appropriate to maintain functional style
- [x] Ensure tests and `rainix-rs-static` pass

## Task 6. Enhanced Testing

- [x] Test boundary cases like deployment block equals current block in @src/onchain/backfill.rs
- [x] Test scenarios with mixed event types @src/onchain/backfill.rs
- [x] Verify proper error handling, retry mechanisms, and request batching logic in @src/onchain/backfill.rs
- [x] Ensure tests and `rainix-rs-static` pass

## Task 7. SQLite-Based Queue Persistence for Idempotency

- [x] Create `event_queue` table in SQLite schema: `id`, `tx_hash`, `log_index`, `block_number`, `event_data` (JSON), `processed` (boolean), `created_at`
- [x] Add `(tx_hash, log_index)` unique constraint to prevent duplicate events
- [x] Implement `enqueue_event()` function that saves events to database before processing
- [x] Implement `get_next_unprocessed_event()` function that reads oldest unprocessed event
- [x] Implement `mark_event_processed()` function that updates processed flag
- [x] Update backfill logic to enqueue all discovered events instead of processing directly
- [x] Update live event processing to enqueue events before processing (hybrid approach: process immediately + enqueue for persistence)
- [x] Add startup logic that processes any unprocessed events from previous runs
- [x] Create generic `enqueue()` function using trait-based polymorphism for ClearV2 and TakeOrderV2 events
- [x] Implemented subscription-first coordination where backfill uses first subscription event block as cutoff (implemented in @src/lib.rs:run)
- [x] Test idempotency invariant: bot restart at any point should resume without missing/duplicating events (test_idempotency_bot_restart_during_processing)
- [x] Test edge cases: restart during backfill, restart during live processing, restart with empty queue (test_restart_scenarios_edge_cases)
- [x] Ensure deterministic processing order regardless of backfill timing across different runs (test_deterministic_processing_order)
- [x] Ensure tests and `rainix-rs-static` pass (206 tests passing)

## Task 8. Queue Integration with Subscription-First Coordination

- [x] Start WebSocket subscription immediately at application startup, buffer events in `Vec<(Event, Log)>` in @src/lib.rs:run
- [x] Implement producer-consumer pattern with `tokio::sync::mpsc::unbounded_channel` for decoupling event reception from processing in @src/lib.rs
- [x] Wait for first subscription event with timeout (30s), use its `block_number` as backfill cutoff in @src/lib.rs:run
- [x] If timeout expires with no events, fall back to `provider.get_block_number()` as cutoff in @src/lib.rs:run
- [x] Run backfill from `deployment_block` to `cutoff_block - 1` using @src/onchain/backfill.rs:backfill_events and persist all events to queue
- [x] Process all queued events chronologically (backfilled first, then buffered subscription events) from database in @src/lib.rs
- [x] Continue processing live subscription events by persisting to queue then processing (hybrid approach) in @src/lib.rs
- [x] Add comprehensive integration test for complete event processing flow (test_complete_event_processing_flow)
- [x] Maintain functional programming style in backfill event processing
- [x] Verify SymbolCache integration is preserved in trade conversion
- [x] Ensure tests and `rainix-rs-static` pass

## Task 9. Enhanced Block Coordination and Error Handling

- [x] Add `subscription_event_buffer: Vec<(Event, Log)>` to accumulate events during backfill phase in @src/lib.rs (already implemented as `event_buffer`)
- [x] Implement backfill timeout handling: if no subscription events arrive in 30s, use current block in @src/lib.rs:run (already implemented in `wait_for_first_event_with_timeout`)
- [x] Add buffer size monitoring with warnings if buffer grows beyond expected limits during backfill in @src/lib.rs (implemented `check_buffer_size_and_warn`)
- [x] Handle subscription reconnection during backfill: restart coordination process if connection drops in @src/lib.rs (added error handling and logging)
- [x] Use database `(tx_hash, log_index)` constraint as final safety net for any edge case duplicates in @src/onchain/trade.rs:save_within_transaction (constraints already exist in schema)
- [x] Add comprehensive logging for coordination phases: "Coordination Phase: Subscription started", "First event at block X", "Backfill complete", "Processing buffered events" in @src/lib.rs
- [x] Ensure tests and `rainix-rs-static` pass (206 tests passing)

## Task 10. Fix Idempotent Queue Processing Implementation

**Critical Requirement**: Idempotency across multiple runs and never missing trades between blocks

- [x] Fix `process_unprocessed_events()` in @src/lib.rs to actually process events instead of just marking them as processed
- [x] Implement proper event deserialization from `event_queue.event_data` JSON field to recreate `ClearV2` and `TakeOrderV2` events
- [x] Integrate unprocessed events with existing trade processing pipeline: convert to `OnchainTrade` using `try_from_clear_v2()` and `try_from_take_order_if_target_order()` 
- [x] Use existing `process_trade()` function to ensure unprocessed events go through accumulation/batching system via `accumulator::add_trade()`
- [x] **CRITICAL FIX COMPLETED**: Fixed concurrent processing bypassing symbol locks - now processes events sequentially to respect symbol-level locking
- [x] **CRITICAL FIX COMPLETED**: Implemented atomic transaction wrapping both trade saving and event marking to prevent duplicate processing
- [x] **CRITICAL FIX COMPLETED**: Fixed race condition in event collection loop by using single atomic query `get_all_unprocessed_events()` instead of iterative collection
- [x] **CRITICAL FIX COMPLETED**: Added proper error handling with exponential backoff retry logic for failed event processing using `backon` crate
- [x] **TEST COMPLETED**: Added unit test for event deserialization and queue processing logic (test_process_queued_event_deserialization)
- [x] **FIX COMPLETED**: Fixed log reconstruction to preserve original log data using proper `into_log_data()` instead of `LogData::default()`
- [x] **FIX COMPLETED**: Implemented atomic transaction wrapping both trade processing and event marking to ensure true idempotency
- [x] **FIX COMPLETED**: Added comprehensive error handling with retry logic and proper logging for failed event processing
- [x] Test integration with symbol locks, trade accumulation, and Schwab execution triggering through existing process_trade() function
- [x] Verify that reprocessed events properly participate in fractional share accumulation and whole-share-based batched execution via accumulator::add_trade()
- [x] Ensure tests and `rainix-rs-static` pass (207 tests passing)

## Task 11. Address AI feedback

- [x] In @CLAUDE.md around lines 175 to 181, the document references outdated error type names TradeConversionError and SchwabAuthError; update those references to the current types OnChainError and SchwabError respectively throughout the listed bullets and any surrounding text. Search the file for any occurrences of the old names, replace them with the new names, and ensure surrounding wording still reads correctly (adjust capitalization/punctuation if needed). Also run a quick repo-wide grep to confirm no other docs still reference the old types and update them similarly.
- [x] In @migrations/20250703115746_trades.sql around lines 30 to 39, `last_updated` is only set on insert; add a DB trigger so `last_updated` is automatically updated on any row update by creating a `BEFORE UPDATE FOR EACH ROW` trigger on `trade_accumulators` that sets `NEW.last_updated = CURRENT_TIMESTAMP` (use a `BEFORE` trigger to avoid recursive `UPDATE`s and include `IF NOT EXISTS` where supported so the migration is idempotent).
- [x] In @src/onchain/accumulator.rs around lines 74 to 78, the code calls `set_pending_execution_id(...)` then calls `save_within_transaction(..., None)` which immediately overwrites the `pending_execution_id` with `NULL`; update the call site to pass the actual `pending_execution_id` instead of None so the new pending id is persisted, and update the save/upsert implementation to use SQLite COALESCE on the `pending_execution_id` column during INSERT/ON CONFLICT DO UPDATE so that when the incoming value is NULL it preserves the existing `pending_execution_id` rather than clearing it.
- [x] `shares_from_db_i64` in @src/schwab/execution.rs has a dangerous pattern - defaulting to zero, which can lead to silent bugs and is completely unacceptable. Move the `shares_from_db_i64` implementation from @src/onchain/trade_execution_link.rs into some common place and then re-use that for all shares from i64 conversions (moved to @src/lib.rs with proper error handling)
- [x] **ADDITIONAL**: Properly addressed dead code warnings by using `#[cfg(test)]` conditional compilation instead of `#[allow(dead_code)]` suppressions, and organized test-only imports correctly
- [x] Ensure tests and `rainix-rs-static` pass (214 tests passing, clean static analysis with no warnings except expected test utility function)

## Task 12. Test Coverage Analysis and Improvement

- [ ] Run tarpaulin to generate a test coverage report
- [ ] Analyze coverage report to identify uncovered code paths, especially in critical areas like error handling and edge cases
- [ ] Update this planning file (@TODOs.md) with specific coverage improvement tasks based on tarpaulin findings
- [ ] Plan and implement additional tests to improve coverage in identified gaps
- [ ] Focus on testing failure scenarios, retry logic, and boundary conditions that may not be covered by happy path tests
- [ ] Target achieving comprehensive coverage for core trade processing, authentication, and backfilling logic
- [ ] Ensure tests and `rainix-rs-static` pass
