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
- [x] **Fix test quality issues identified during code review (PROPER APPROACH):**
  - [x] Refactor `process_tx_command_with_writers` to accept an alloy Provider parameter for proper dependency injection via new `process_tx_with_provider` function
  - [x] Fix `test_process_tx_with_database_integration_success` - now uses Alloy's mock Provider to properly test successful blockchain data retrieval, trade processing, and Schwab API calls
  - [x] Fix `test_process_tx_database_duplicate_handling` - now uses Alloy's mock Provider to test duplicate transaction handling with proper trade saving and detection
  - [x] Fix `test_integration_buy_command_end_to_end` - properly tests full CLI argument parsing → `run_command_with_writers` → order execution workflow with HTTP mocking
  - [x] Fix `test_integration_sell_command_end_to_end` - properly tests full CLI argument parsing → `run_command_with_writers` → order execution workflow with HTTP mocking
  - [x] **Key architectural improvements:** 
    - Refactored `run_command_with_writers` to accept database connection parameter, enabling proper dependency injection for tests
    - Created `process_tx_with_provider` function that accepts Provider and SymbolCache for proper mocking
    - Removed redundant `run_with_writers` function
  - **NOTE: Tests now properly test what their names claim to test, with no shortcuts - using Alloy's built-in Provider mocking**
- [x] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt` - All 163 tests passing with proper mocking

### 2A. Fix Fundamental Schema Design Issues ✅ COMPLETED (BUT STILL WRONG!)

**CRITICAL: The "corrected" schema is still fundamentally flawed for fractional shares:**

**Problem with Current "Fixed" Schema:**
The current direct foreign key approach assumes 1:1 relationship between onchain trades and Schwab executions, but **fractional onchain trades require many-to-one relationship** with whole-share Schwab executions.

**Example showing the problem:**
- Onchain trade 1: 1.1 AAPL → Should create Schwab execution for 1 share + leave 0.1 unexecuted
- Onchain trade 2: 0.8 AAPL → Should accumulate to 0.9 total unexecuted + still wait  
- Onchain trade 3: 0.2 AAPL → Should accumulate to 1.1 total + create another 1-share execution

**Wrong current approach:** Each onchain trade gets linked to exactly one execution
**Correct approach:** Multiple onchain trades can contribute fractional amounts to one execution

### 2B. Fix Schema for Proper Fractional Share Handling ✅ COMPLETED (BUT NEEDS REFACTORING)

**CRITICAL: Junction table approach was implemented but creates architectural problems:**

**What was completed:**
- [x] **Restored `trade_executions` junction table** with `executed_amount` field
- [x] **Removed direct `schwab_execution_id` foreign key** from onchain_trades
- [x] **Updated database methods** to handle partial execution of trades
- [x] **Implemented fractional accumulation logic** in position accumulator
- [x] **Updated coordinator** to handle partial executions with complex allocation logic

**Problem Identified:** The current architecture separates database linking logic (`TradeExecutionLink`) from business logic (accumulation and batching), creating cognitive overhead and making the code harder to reason about. The coordinator contains complex nested logic for finding trades, calculating unexecuted amounts, and creating links.

### 2C. Refactor to Batch-Centric Architecture (NEW PRIORITY)

**Goal:** Replace fragmented architecture with a `TradeBatch` domain model that encapsulates fractional share accumulation and execution logic in a single, coherent concept.

**Root Cause of Current Issues:**
- Database operations divorced from business logic
- Complex coordinator with nested allocation logic  
- Need to understand 3 separate components (TradeExecutionLink, PositionAccumulator, TradeCoordinator) to understand fractional shares
- Missing domain model for "batch of trades that accumulate to whole-share execution"

**New Architecture Requirements:**

- [ ] **Create TradeBatch domain model** with business methods and clear state transitions
- [ ] **Replace TradeExecutionLink** with TradeBatch-centric database operations
- [ ] **Update database schema** to use `trade_batches` and `batch_trades` tables
- [ ] **Simplify coordinator logic** to work with batch operations instead of complex allocation
- [ ] **Update position accumulator** to only handle threshold checking, not execution tracking
- [ ] **Implement clear state machine**: New → Accumulating → Ready → Executing → Completed

**New Batch-Centric Schema Design:**
```sql
-- Onchain trades remain immutable facts
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,  -- Can be fractional (e.g., 1.1 shares)
  price_usdc REAL NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  UNIQUE (tx_hash, log_index)
);

-- Schwab executions are whole-share API calls
CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL,  -- Always whole numbers (Schwab limitation)
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL,
  executed_at TIMESTAMP
);

-- Trade batches represent accumulations of fractional trades toward whole-share executions
CREATE TABLE trade_batches (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  target_shares INTEGER NOT NULL,  -- Whole shares this batch will execute
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  current_amount REAL NOT NULL DEFAULT 0.0,  -- Current accumulated fractional amount
  status TEXT CHECK (status IN ('ACCUMULATING', 'READY', 'EXECUTING', 'COMPLETED', 'FAILED')) NOT NULL DEFAULT 'ACCUMULATING',
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),  -- Set when executed
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  completed_at TIMESTAMP
);

-- Junction table tracking which trades contribute to which batches
CREATE TABLE batch_trades (
  batch_id INTEGER REFERENCES trade_batches(id),
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  contributed_amount REAL NOT NULL,  -- How much of the trade was contributed to this batch
  PRIMARY KEY (batch_id, onchain_trade_id)
);

-- Position tracking simplified to just threshold checking
CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,  -- For threshold checking only
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

**Example Flow for Fractional Shares:**
1. Onchain trade: 1.1 AAPL → position_accumulator shows +1.1
2. Threshold reached → Create Schwab execution for 1 share
3. Junction table: links trade to execution with executed_amount=1.0
4. Position accumulator updated to +0.1 (remaining fraction)
5. Later onchain trade: 0.8 AAPL → position accumulator shows +0.9
6. Another trade: 0.2 AAPL → position accumulator shows +1.1 → trigger new execution

**Benefits of Corrected Design:**
- ✅ **Handles fractional shares correctly**: Multiple trades can contribute to one execution
- ✅ **Tracks partial executions**: Each trade can be partially executed across multiple Schwab calls
- ✅ **Maintains audit trail**: Junction table shows exactly how much of each trade was executed when
- ✅ **Supports accumulation**: Fractional remainders properly accumulate for future executions
- ✅ **Database integrity**: Proper foreign key relationships without incorrect 1:1 assumptions

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

**Current State:** Task 2A was completed but revealed a deeper design flaw. The schema was "corrected" to use direct foreign keys, but this is still wrong for fractional shares. All 167 tests pass with the current approach, but it cannot handle partial execution of fractional onchain trades.

**Critical Realization:** The original junction table approach was actually correct! The problem wasn't the many-to-many relationship - it was the status-based thinking. We need the junction table to track partial execution of fractional shares, but WITHOUT status transitions on onchain trades.

**Next Step:** Complete Task 2B - Implement proper fractional share handling by restoring the junction table with `executed_amount` field, removing the direct foreign key, and implementing proper partial execution logic.

---

## Schema Design (Target Architecture)

The corrected schema properly handles fractional shares with many-to-one relationships:

```sql
-- FINAL CORRECT SCHEMA for fractional share accumulation

-- Onchain trades are immutable blockchain facts
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,  -- Can be fractional (e.g., 1.1 shares)
  price_usdc REAL NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  UNIQUE (tx_hash, log_index)
  -- No status field - these are immutable blockchain facts
  -- No direct foreign key - multiple trades can contribute to one execution
);

-- Schwab executions are whole-share API calls
CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL,  -- Always whole numbers (Schwab API limitation)
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL,
  executed_at TIMESTAMP
  -- No DEFAULT CURRENT_TIMESTAMP - set only when actually executed
);

-- Junction table tracking fractional contributions
CREATE TABLE trade_executions (
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  executed_amount REAL NOT NULL,  -- How much of the onchain trade was used
  PRIMARY KEY (onchain_trade_id, schwab_execution_id)
);

-- Position tracking with fractional precision
CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,  -- Includes unexecuted fractions
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

**Key Design Principles:**
- **Onchain trades are immutable facts** - no status changes, no direct execution links
- **Schwab executions handle whole shares only** - API limitation drives design
- **Junction table enables fractional tracking** - one trade can contribute to multiple executions
- **Position accumulator tracks remainders** - fractional amounts accumulate until executable

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
