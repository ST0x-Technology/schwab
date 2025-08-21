# Visibility Level Refactoring Plan

This plan completes the refactoring to reduce visibility levels across the
project to the maximum needed ones. The purpose is to get more help from the
compiler and linter by helping them identify dead code and as a result to make
it easier to navigate the code (by having less of it) and easier to understand
(by making the relevance scope explicit).

## Task 1. Fix Unused Imports from Visibility Changes

- [x] Remove `AccountNumbers` from schwab/mod.rs:13
- [x] Remove `SchwabAuthResponse` from schwab/mod.rs:13
- [x] Remove `execution::SchwabExecution` from schwab/mod.rs:14 (now unused
      after visibility reduction)
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 2. Remove Dead Code Exposed by Visibility Reduction

- [x] Remove `ExecutionIdMismatch` variant from PersistenceError enum in
      src/error.rs:49-55 (never constructed)
- [x] Remove `clear_pending_execution_within_transaction` function from
      src/lock.rs:64 (never used)
- [x] Add `#[allow(dead_code)]` to unused SchwabExecutionBuilder methods or
      remove them
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 3. Make SchwabExecution Visible to Other Modules

- [x] Change `SchwabExecution` struct back to `pub(crate)` in
      schwab/execution.rs
- [x] Change `update_execution_status_within_transaction` back to `pub(crate)`
      if needed by other modules
- [x] Change `find_execution_by_id` back to `pub(crate)` if needed by other
      modules
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 4. Reduce Error Module Visibility

- [x] Change `pub enum TradeValidationError` to
      `pub(crate) enum TradeValidationError`
- [x] Change `pub enum PersistenceError` to `pub(crate) enum PersistenceError`
- [x] Change `pub enum AlloyError` to `pub(crate) enum AlloyError`
- [x] Change `pub enum EventQueueError` to `pub(crate) enum EventQueueError`
- [x] Change `pub enum OnChainError` to `pub(crate) enum OnChainError`
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 5. Reduce Conductor Module Visibility

- [x] Change `pub async fn get_cutoff_block` to `pub(crate)` if only used
      internally
- [x] Change `pub async fn run_live` to `pub(crate)` if only used internally
- [x] Change `pub async fn process_queue` to `pub(crate)` if only used
      internally
- [x] Keep functions as `pub` only if used by bin files
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 6. Reduce Lock Module Visibility

- [x] Change `pub async fn try_acquire_execution_lease` to `pub(crate)`
- [x] Change `pub async fn clear_execution_lease` to `pub(crate)`
- [x] Change `pub async fn set_pending_execution_id` to `pub(crate)`
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 7. Reduce Queue Module Visibility

- [x] Change `pub async fn get_next_unprocessed_event` to `pub(crate)`
- [x] Change `pub async fn mark_event_processed` to `pub(crate)`
- [x] Change `pub async fn count_unprocessed` to `pub(crate)`
- [x] Change `pub async fn get_all_unprocessed_events` to `pub(crate)`
- [x] Change `pub async fn enqueue` to `pub(crate)`
- [x] Change `pub async fn enqueue_buffer` to `pub(crate)`
- [x] Keep `Enqueueable` trait and `QueuedEvent` struct as `pub` if used
      externally
- [x] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [x] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 8. Reduce Onchain Submodule Visibility

- [ ] Change `pub mod accumulator` to `pub(crate) mod accumulator`
- [ ] Change `pub mod position_calculator` to
      `pub(crate) mod position_calculator`
- [ ] Change `pub mod trade` to `pub(crate) mod trade`
- [ ] Change `pub mod trade_execution_link` to
      `pub(crate) mod trade_execution_link`
- [ ] Keep `pub use trade::OnchainTrade` if used by other modules
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 9. Reduce Symbol Module Visibility

- [ ] Change `pub mod cache` to `pub(crate) mod cache` in symbol/mod.rs
- [ ] Change `pub mod lock` to `pub(crate) mod lock` in symbol/mod.rs
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 10. Review lib.rs Exports

- [ ] Check which modules are used by bin/main.rs, bin/auth.rs, bin/cli.rs
- [ ] Change unused `pub mod` declarations to `pub(crate) mod`
- [ ] Keep only necessary public exports for the binary interfaces
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and additional plans for anything that needs
      fixing

## Task 11. Final Verification and Cleanup

- [ ] Run full test suite: `cargo test`
- [ ] Run static analysis: `rainix-rs-static`
- [ ] Run pre-commit hooks: `pre-commit run -a`
- [ ] Verify no unused import or dead code warnings remain
- [ ] Mark all tasks as completed in PLAN.md
- [ ] Delete PLAN.md

## Progress Summary

1. [x] Fix Unused Imports from Visibility Changes
2. [x] Remove Dead Code Exposed by Visibility Reduction
3. [x] Make SchwabExecution Visible to Other Modules
4. [x] Reduce Error Module Visibility
5. [x] Reduce Conductor Module Visibility
6. [x] Reduce Lock Module Visibility
7. [x] Reduce Queue Module Visibility
8. [ ] Reduce Onchain Submodule Visibility
9. [ ] Reduce Symbol Module Visibility
10. [ ] Review lib.rs Exports
11. [ ] Final Verification and Cleanup

## Current Status

Task 1 and 2 completed successfully. Removed dead code including:

- ExecutionIdMismatch variant from PersistenceError enum
- clear_pending_execution_within_transaction function and associated test
- Unused SchwabExecutionBuilder methods (with_shares, with_direction,
  with_status, with_symbol)

Fixed clippy issue with trivially_copy_pass_by_ref in Direction::as_str method.
All tests pass, static analysis passes, and pre-commit hooks pass.
