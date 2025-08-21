# Visibility Reduction Refactoring Plan

## Overview

This plan outlines the systematic reduction of visibility levels across the
codebase to the minimum required levels. The goal is to leverage the compiler
and linter to identify dead code, improve code navigation, and make relevance
scope explicit.

## Task 1: Fix Clippy Errors from redundant_pub_crate

- [ ] Change `pub(crate)` to `pub` in schwab/execution.rs for:
  - [ ] `SchwabExecution` struct
  - [ ] `update_execution_status_within_transaction` function
  - [ ] `find_execution_by_id` function
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 2: Remove Unused Imports in schwab/mod.rs

- [ ] Remove unused import `AccountNumbers` from schwab/mod.rs:13
- [ ] Remove unused import `SchwabAuthResponse` from schwab/mod.rs:13
- [ ] Remove unused import `execution::SchwabExecution` from schwab/mod.rs:14
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 3: Handle ExecutionIdMismatch Variant

- [ ] Search for usages of ExecutionIdMismatch in the codebase
- [ ] Either:
  - [ ] Remove the variant if unused, OR
  - [ ] Add a use case for the variant where appropriate
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 4: Remove Unused Function clear_pending_execution_within_transaction

- [ ] Verify function is not used anywhere in the codebase
- [ ] Remove `clear_pending_execution_within_transaction` from lock.rs:64
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 5: Fix Struct Field Naming in schwab/order.rs

- [ ] Rename `order_type` to `type` in Order struct
- [ ] Rename `order_strategy_type` to `strategy_type` in Order struct
- [ ] Rename `order_leg_collection` to `leg_collection` in Order struct
- [ ] Update all references to these fields throughout the codebase and make
      sure remaining issues are handled in the plan
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 6: Fix Trivial Pass-by-Reference

- [ ] Change `Direction::as_str(&self)` to `Direction::as_str(self)` in
      schwab/mod.rs:61
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 7: Reduce Visibility in Error Module

- [ ] Change all `pub enum` to `pub(crate) enum` in src/error.rs for:
  - [ ] TradeValidationError
  - [ ] PersistenceError
  - [ ] AlloyError
  - [ ] EventQueueError
  - [ ] OnChainError
- [ ] Update all imports that reference these types
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 8: Review and Reduce Conductor Module Visibility

- [ ] Check if `get_cutoff_block` is used outside conductor module
- [ ] Check if `run_live` is used outside conductor module
- [ ] Check if `process_queue` is used outside conductor module
- [ ] Reduce visibility to pub(crate) or private where appropriate
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 9: Reduce Visibility in Lock Module

- [ ] Change all `pub async fn` to `pub(crate) async fn` in src/lock.rs for:
  - [ ] try_acquire_execution_lease
  - [ ] clear_execution_lease
  - [ ] set_pending_execution_id
- [ ] Update imports in other modules that use these functions
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 10: Review Queue Module Visibility

- [ ] Check usage of each pub function in queue.rs
- [ ] Change to pub(crate) for:
  - [ ] get_next_unprocessed_event
  - [ ] mark_event_processed
  - [ ] count_unprocessed
  - [ ] get_all_unprocessed_events
  - [ ] enqueue
  - [ ] enqueue_buffer
- [ ] Keep Enqueueable trait and QueuedEvent as pub if needed by external
      modules
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 11: Reduce Symbol Module Visibility

- [ ] Change `pub mod cache` to `pub(crate) mod cache` in symbol/mod.rs
- [ ] Change `pub mod lock` to `pub(crate) mod lock` in symbol/mod.rs
- [ ] Update imports in other modules
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 12: Review Onchain Submodule Visibility

- [ ] Change `pub mod accumulator` to `pub(crate) mod accumulator`
- [ ] Change `pub mod position_calculator` to
      `pub(crate) mod position_calculator`
- [ ] Change `pub mod trade_execution_link` to
      `pub(crate) mod trade_execution_link`
- [ ] Change `pub mod backfill` to `pub(crate) mod backfill`
- [ ] Change `pub mod trade` to `pub(crate) mod trade`
- [ ] Keep `pub use trade::OnchainTrade` if needed externally
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 13: Clean Up Schwab Module

- [ ] Remove SchwabInstruction type alias (use Direction directly)
- [ ] Update all references from SchwabInstruction to Direction
- [ ] Review if auth, execution, order, tokens submodules can be private
- [ ] Clean up unnecessary re-exports
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 14: Review lib.rs Exports

- [ ] Check which modules are actually used by bin/main.rs, bin/auth.rs,
      bin/cli.rs
- [ ] Change unnecessary `pub mod` to `pub(crate) mod` or private
- [ ] Document why each public export is needed
- [ ] Run `cargo test -q && rainix-rs-static`
- [ ] Update PLAN.md with progress and make sure remaining issues are handled in
      the plan

## Task 15: Final Verification

- [ ] Run full test suite: `cargo test`
- [ ] Run clippy with all checks:
      `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run static analysis: `rainix-rs-static`
- [ ] Run pre-commit hooks: `pre-commit run -a`
- [ ] Update PLAN.md marking all completed tasks
- [ ] Delete PLAN.md

## Progress Summary

**Total Tasks:** 15\
**Completed:** 0\
**In Progress:** 0\
**Remaining:** 15

---

_Last Updated:_ 2025-08-21
