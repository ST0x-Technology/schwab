# Implementation Plan: Trade Accumulation and Batching for Fractional Shares

Based on the design decision that Schwab API doesn't support fractional shares but our onchain tokenized stocks do, this plan implements a proper separation between onchain trades and Schwab executions with a many-to-one relationship.

**REVISED APPROACH: Incremental Migration Strategy**

The previous approach of replacing the entire schema at once caused too many breaking changes. This revised plan moves database queries out of tests into helper functions, then implements clean cut-over to new schema without dual writes.

## Task 0. Move Database Queries Out of Tests (Foundation) ✅ COMPLETED

**Strategy:** Ensure tests don't contain direct SQL queries, but instead use helper functions/methods. This allows schema changes without breaking all tests.

- [x] Identify all tests that contain direct `sqlx::query!` calls
- [x] Create helper functions or extend existing struct methods to encapsulate database operations:
  - `ArbTrade::db_count()`, `ArbTrade::find_by_tx_hash_and_log_index()`, etc.
  - `SchwabTokens::db_count()`, etc.
  - Removed overly specific helper methods that weren't reusable
- [x] Update all tests to use these helper functions instead of direct SQL
- [x] Ensure all business logic already uses struct methods (most already does)
- [x] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

## Task 1. Add New Tables Alongside Existing Schema ✅ COMPLETED

**NOTE: We modify the existing migration file directly instead of creating new migrations for simplicity**

- [x] Create migration that ADDS new tables without touching existing `trades` table:
  - `onchain_trades` table for blockchain events
  - `schwab_executions` table for Schwab API calls  
  - `position_accumulator` table for running positions
  - `trade_executions` linkage table for many-to-one relationships
- [x] Verify migration runs successfully: `sqlx migrate run`
- [x] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

## Task 2. Create New Schema Structs and Methods

- [ ] Create new Rust data structures for new tables:
  - `OnchainTrade` struct with database methods
  - `SchwabExecution` struct with database methods
  - `ExecutablePosition` struct for batching logic
  - `position_accumulator` module with helper functions
- [ ] Implement database methods for new structs (similar to existing `ArbTrade` methods):
  - `OnchainTrade::save()`, `OnchainTrade::find()`, `OnchainTrade::count()`, etc.
  - `SchwabExecution::save()`, `SchwabExecution::find()`, etc.
  - Position accumulator helper functions
- [ ] Add unit tests for new structs using helper functions (not direct SQL)
- [ ] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

## Task 3. Replace Old System with New System

- [ ] Remove old structs (`ArbTrade`, etc.) and their methods
- [ ] Update main processing logic to use new batching workflow:
  - Parse events → `OnchainTrade` → Position accumulation → `SchwabExecution` when threshold reached
- [ ] Update CLI and other components to use new schema
- [ ] Add integration tests that verify new system works correctly
- [ ] Test that position accumulation and batching logic works as expected
- [ ] Test edge cases: duplicate events, failed executions, bot restarts
- [ ] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

## Task 4. Schema Cleanup

**NOTE: We modify the existing migration file directly instead of creating new migrations for simplicity**

- [ ] Modify existing migration to remove old `trades` table definition
- [ ] Remove old struct definitions and database methods
- [ ] Clean up any temporary configuration options
- [ ] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

**Benefits of Incremental Approach:**
- ✅ Tests focus on business logic, not database schema details
- ✅ Schema changes only require updating helper functions/methods
- ✅ Code remains working and testable at each step
- ✅ Clean cut-over without dual-write complexity
- ✅ Easy rollback if issues discovered
- ✅ Gradual migration reduces risk
- ✅ Tests guide migration process without constant updates
- ✅ Simple and pragmatic approach

**Risk Mitigation:**
- Each phase is independently deployable and testable
- Rollback procedures defined for each phase
- Data integrity checks at each step
- Comprehensive testing before proceeding to next phase
- Old system remains functional until new system proven stable

## Current Status

**Current State:** Working codebase with both old and new schemas available. All existing functionality works (138 tests passing), and new tables are ready for incremental implementation.

**Next Step:** Begin Task 2 - Create new schema structs with database methods. The foundation from Task 0 is complete, making the schema cut-over much easier.

---

## Schema Design (Target Architecture)

The new schema properly separates onchain trades from Schwab executions:

```sql
-- New tables to be added (Phase 1A)
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,
  price_usdc REAL NOT NULL,
  status TEXT CHECK (status IN ('PENDING', 'ACCUMULATED', 'EXECUTED')),
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL,
  direction TEXT CHECK (direction IN ('BUY', 'SELL')),
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')),
  executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE trade_executions (
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  PRIMARY KEY (onchain_trade_id, schwab_execution_id)
);
```
