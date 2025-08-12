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

## Task 2. Create New Schema Structs and Methods ✅ COMPLETED

- [x] Create new Rust data structures for new tables:
  - `OnchainTrade` struct with database methods in `src/onchain/trade.rs`
  - `SchwabExecution` struct with database methods in `src/schwab/execution.rs`
  - `ExecutablePosition` struct for batching logic in `src/onchain/position_accumulator.rs`
  - `position_accumulator` module with helper functions
  - `TradeExecutionLink` struct for many-to-one relationships in `src/onchain/trade_executions.rs`
- [x] Implement database methods for new structs (similar to existing `ArbTrade` methods):
  - `OnchainTrade::save()`, `OnchainTrade::try_save()`, `OnchainTrade::find_by_tx_hash_and_log_index()`, `OnchainTrade::find_by_symbol_and_status()`, `OnchainTrade::update_status()`, `OnchainTrade::db_count()`
  - `SchwabExecution::save()`, `SchwabExecution::find_by_id()`, `SchwabExecution::find_by_symbol_and_status()`, `SchwabExecution::update_status_and_order_id()`, `SchwabExecution::db_count()`
  - Position accumulator helper functions: `PositionAccumulator::get_or_create()`, `PositionAccumulator::update_position()`, `PositionAccumulator::should_execute()`, `accumulate_onchain_trade()`
  - Trade execution linkage: `TradeExecutionLink::create_link()`, `TradeExecutionLink::find_onchain_trades_for_execution()`, `TradeExecutionLink::find_executions_for_onchain_trade()`
  - **CRITICAL: Applied same strict parsing patterns as `ArbTrade::find_by_tx_hash_and_log_index()`:**
    - No `.unwrap_or()` default values for status/direction parsing
    - Return proper `TradeConversionError` variants for invalid data
    - Fail fast on database corruption instead of masking with defaults
- [x] Add comprehensive unit tests for new structs using helper functions (not direct SQL)
- [x] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt` - All 156 tests passing

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
  status TEXT CHECK (status IN ('PENDING', 'ACCUMULATED', 'EXECUTED')) NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL,
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL,
  executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE trade_executions (
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  PRIMARY KEY (onchain_trade_id, schwab_execution_id)
);
```

## Task 5. Refactor TradeStatus to Sophisticated Enum

**Strategy:** Replace the current simple `TradeStatus` enum with sophisticated variants that bundle related data, improving type safety without requiring database schema changes.

**Current State:**
```rust
pub enum TradeStatus {
    Pending,
    Completed,
    Failed,
}

pub struct SchwabExecution {
    // ... other fields
    pub status: TradeStatus,
    pub order_id: Option<String>,      // Separate nullable field
    pub price_cents: Option<u64>,      // Separate nullable field  
    pub executed_at: Option<String>,   // Separate nullable field
}
```

**Target State:**
```rust
pub enum TradeStatus {
    Pending,
    Completed { 
        executed_at: DateTime<Utc>, 
        order_id: String, 
        price_cents: u64 
    },
    Failed { 
        failed_at: DateTime<Utc>, 
        error_reason: Option<String> 
    },
}

pub struct SchwabExecution {
    // ... other fields
    pub status: TradeStatus,  // Contains all status-related data
    // Remove: order_id, price_cents, executed_at fields
}
```

- [ ] Update TradeStatus enum in `src/onchain/mod.rs` with sophisticated variants
- [ ] Update SchwabExecution struct in `src/schwab/execution.rs` to remove separate status fields  
- [ ] Implement custom SQLx serialization to map enum variants to database columns:
  - `Pending` → `status="PENDING"`, other fields NULL
  - `Completed{...}` → `status="COMPLETED"` + field values
  - `Failed{...}` → `status="FAILED"` + timestamp
- [ ] Update all usage sites in `src/onchain/coordinator.rs` to use pattern matching instead of field access
- [ ] Update database operations (`save_within_transaction`, `update_status_within_transaction`) to extract/construct enum variants properly
- [ ] Update all test files using SchwabExecution to use pattern matching in assertions
- [ ] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

**Benefits of Sophisticated Enum:**
- ✅ **Type Safety**: Impossible to have `Completed` status without required data (order_id, price_cents, executed_at)
- ✅ **Cleaner API**: Single status field contains all relevant data instead of separate nullable fields
- ✅ **Better Semantics**: Business rules enforced by type system at compile time
- ✅ **No Schema Changes**: Uses existing database columns with custom serialization
- ✅ **Impossible States**: Cannot represent invalid combinations like `Pending` with `order_id`
