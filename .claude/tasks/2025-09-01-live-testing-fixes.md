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

**Issue**: Executions stuck in SUBMITTED state cause deadlocks **Verification**:
No existing cleanup mechanism; `pending_execution_id` blocks new executions
**Files**: `src/onchain/accumulator.rs` (clean_up_stale_executions function)

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Add test coverage for stale execution cleanup scenarios
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`

## Task 6: Improved Logging

**Issue**: Insufficient logging for debugging production issues
**Verification**: Additional info! statements for observability **Files**:
`src/conductor.rs`

- [ ] Apply changes from stash
- [ ] Review implementation
- [ ] Verify logging coverage is adequate
- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`
