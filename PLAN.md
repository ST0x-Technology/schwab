# Visibility Level Refactoring Plan

This plan completes the refactoring to reduce visibility levels across the
project to the maximum needed ones. The purpose is to get more help from the
compiler and linter by helping them identify dead code and as a result to make
it easier to navigate the code (by having less of it) and easier to understand
(by making the relevance scope explicit).

## Phase 1: Fix Immediate Blocking Errors

### Task 1: Fix Order Struct Field Naming

- [ ] Change `order_type` to `type` in Order struct (src/schwab/order.rs:29)
- [ ] Change `order_strategy_type` to `strategy_type` in Order struct
      (src/schwab/order.rs:32)
- [ ] Change `order_leg_collection` to `leg_collection` in Order struct
      (src/schwab/order.rs:33)
- [ ] Update constructor usage in Order::new function
- [ ] Update all test assertions that reference these fields
- [ ] Update serialization tests that check field values
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 2: Fix Direction as_str Method Signature

- [ ] Change `pub const fn as_str(&self)` to `pub(crate) const fn as_str(self)`
      in src/schwab/mod.rs:61
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 3: Remove Unused Imports

- [ ] Remove `AccountNumbers` from schwab/mod.rs:13
- [ ] Remove `SchwabAuthResponse` from schwab/mod.rs:13
- [ ] Remove `execution::SchwabExecution` from schwab/mod.rs:14
- [ ] Verify these types are still accessible via their module paths where
      needed
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 4: Handle ExecutionIdMismatch Variant

- [ ] Check if ExecutionIdMismatch is referenced anywhere in the codebase
- [ ] Remove the variant from PersistenceError enum in src/error.rs:49-55 if
      unused
- [ ] If removal breaks anything, add `#[allow(dead_code)]` instead
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 5: Remove Unused Function

- [ ] Remove `clear_pending_execution_within_transaction` function from
      src/lock.rs:64
- [ ] Verify no hidden references exist in the codebase
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

## Phase 2: Continue Visibility Reduction

### Task 6: Reduce Error Module Visibility

- [ ] Change `pub enum TradeValidationError` to
      `pub(crate) enum TradeValidationError`
- [ ] Change `pub enum PersistenceError` to `pub(crate) enum PersistenceError`
- [ ] Change `pub enum AlloyError` to `pub(crate) enum AlloyError`
- [ ] Change `pub enum EventQueueError` to `pub(crate) enum EventQueueError`
- [ ] Change `pub enum OnChainError` to `pub(crate) enum OnChainError`
- [ ] Update any imports in other modules that reference these types
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 7: Reduce Conductor Module Visibility

- [ ] Check if `get_cutoff_block` is used outside conductor module
- [ ] Check if `run_live` is used outside conductor module
- [ ] Check if `process_queue` is used outside conductor module
- [ ] Change to `pub(crate)` or private where functions are only used internally
- [ ] Keep `pub` only for functions used by bin files
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 8: Reduce Lock Module Visibility

- [ ] Change `pub async fn try_acquire_execution_lease` to `pub(crate)` if not
      used by bins
- [ ] Change `pub async fn clear_execution_lease` to `pub(crate)` if not used by
      bins
- [ ] Change `pub async fn set_pending_execution_id` to `pub(crate)` if not used
      by bins
- [ ] Update imports in other modules that use these functions
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 9: Reduce Queue Module Visibility

- [ ] Check usage of `get_next_unprocessed_event` function
- [ ] Check usage of `mark_event_processed` function
- [ ] Check usage of `count_unprocessed` function
- [ ] Check usage of `get_all_unprocessed_events` function
- [ ] Check usage of `enqueue` and `enqueue_buffer` functions
- [ ] Change to `pub(crate)` for functions only used internally
- [ ] Keep `Enqueueable` trait and `QueuedEvent` as `pub` if needed by external
      modules
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 10: Clean Up Test Utilities

- [ ] Add `#[allow(dead_code)]` to unused builder methods in
      SchwabExecutionBuilder
- [ ] Or remove unused methods: `with_symbol`, `with_shares`, `with_direction`,
      `with_status`
- [ ] Verify test utilities are still functional for existing tests
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

## Phase 3: Module Organization

### Task 11: Review Onchain Submodules

- [ ] Change `pub mod accumulator` to `pub(crate) mod accumulator`
- [ ] Change `pub mod position_calculator` to
      `pub(crate) mod position_calculator`
- [ ] Change `pub mod trade` to `pub(crate) mod trade`
- [ ] Change `pub mod trade_execution_link` to
      `pub(crate) mod trade_execution_link`
- [ ] Keep necessary re-exports like `pub use trade::OnchainTrade` if used
      externally
- [ ] Update imports in other modules
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 12: Review Symbol Submodules

- [ ] Change `pub mod cache` to `pub(crate) mod cache` in symbol/mod.rs
- [ ] Change `pub mod lock` to `pub(crate) mod lock` in symbol/mod.rs
- [ ] Update imports in other modules that use symbol submodules
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 13: Clean Up Schwab Module

- [ ] Review if SchwabInstruction type alias can be removed (replace with
      Direction)
- [ ] Update all references from SchwabInstruction to Direction throughout
      codebase
- [ ] Review if auth, execution, order, tokens submodules can be made private
- [ ] Clean up unnecessary re-exports in schwab/mod.rs
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

### Task 14: Review lib.rs Exports

- [ ] Check which modules are actually used by bin/main.rs
- [ ] Check which modules are actually used by bin/auth.rs
- [ ] Check which modules are actually used by bin/cli.rs
- [ ] Change unnecessary `pub mod` to `pub(crate) mod` or private
- [ ] Document why each remaining public export is needed
- [ ] Run `cargo test -q && rainix-rs-static && pre-commit run -a`
- [ ] Update PLAN.md with progress and handle any new issues

## Phase 4: Final Verification

### Task 15: Complete Final Checks and Cleanup

- [ ] Run full test suite: `cargo test`
- [ ] Run clippy with all checks:
      `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run static analysis: `rainix-rs-static`
- [ ] Run pre-commit hooks: `pre-commit run -a`
- [ ] Verify no warnings or errors remain
- [ ] Review any new dead code warnings and address them
- [ ] Update PLAN.md marking all completed tasks
- [ ] Delete PLAN.md after successful completion

## Progress Summary

- [ ] Phase 1: Fix Immediate Blocking Errors (Tasks 1-5)
- [ ] Phase 2: Continue Visibility Reduction (Tasks 6-10)
- [ ] Phase 3: Module Organization (Tasks 11-14)
- [ ] Phase 4: Final Verification (Task 15)

## Current Status

Starting refactoring - all tasks pending.
