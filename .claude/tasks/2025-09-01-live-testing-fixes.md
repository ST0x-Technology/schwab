# Live Testing Fixes - September 1, 2025

Four verified fixes discovered during live testing of the Schwab integration.

## Task 1: Schwab API Response Format Fix (CRITICAL)

**Issue**: Schwab returns `orderId` as int64, not string; execution data in
`orderActivityCollection` **Source**:
`account_orders_openapi.yaml:1364,1472,1518,2506` defines
`orderId: type: integer, format: int64` **Files**: `src/schwab/order_status.rs`,
`src/schwab/order.rs`

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`

## Task 2: Duplicate Event Handling

**Issue**: System fails on duplicate events instead of handling gracefully
**Verification**: UNIQUE constraints on `(tx_hash, log_index)` exist; graceful
handling needed for event redelivery **Files**: `src/onchain/accumulator.rs`,
`src/conductor.rs`, `src/cli.rs`

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`

## Task 3: Stale Execution Cleanup

**Issue**: Executions stuck in SUBMITTED state cause deadlocks **Verification**:
No existing cleanup mechanism; `pending_execution_id` blocks new executions
**Files**: `src/onchain/accumulator.rs` (clean_up_stale_executions function)

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`

## Task 4: Improved Logging

**Issue**: Insufficient logging for debugging production issues
**Verification**: Additional info! statements for observability **Files**:
`src/conductor.rs`

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`
