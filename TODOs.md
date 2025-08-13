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

### 2C. Unified Trade Accumulation Architecture ✅ COMPLETED

**CRITICAL REALIZATION:** The batch-centric approach still has the same separation of concerns problem - `PositionAccumulator` for thresholds, `TradeBatch` for state, coordinator for orchestration. This creates cognitive overhead.

**New Goal:** Create a single `TradeAccumulator` domain object that encapsulates ALL fractional share logic in one place:
- Position tracking and threshold checking
- Trade accumulation toward whole-share executions  
- Execution state management and Schwab API calls
- Database persistence of all related state

**Root Cause Analysis:**
- Current approach: `PositionAccumulator` (threshold) + `TradeBatch` (accumulation) + `TradeCoordinator` (orchestration) = 3 concepts to understand
- Target approach: `TradeAccumulator` = 1 concept that handles everything

**Unified Architecture Requirements:**

- [x] **Create TradeAccumulator domain model** that encapsulates:
  - Position tracking (net position for threshold checking)
  - Trade accumulation (fractional amounts toward whole shares)
  - Execution triggering (when ready, create Schwab execution)
  - State transitions (Accumulating → Executing → Completed)
- [x] **Single method interface**: `TradeAccumulator::add_trade(trade) -> Option<SchwabExecution>`
- [x] **Replace 3 separate components** (PositionAccumulator, TradeBatch, complex coordinator) with unified logic
- [x] **Simplified database schema** with fewer tables and clearer relationships

**New Unified Schema Design:**
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

-- Unified trade accumulator - ONE table that tracks everything
CREATE TABLE trade_accumulators (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,  -- Running position for threshold checking
  accumulated_long REAL NOT NULL DEFAULT 0.0,  -- Fractional shares accumulated for buying
  accumulated_short REAL NOT NULL DEFAULT 0.0,  -- Fractional shares accumulated for selling
  pending_execution_id INTEGER REFERENCES schwab_executions(id),  -- Current pending execution if any
  threshold REAL NOT NULL DEFAULT 1.0,  -- Minimum position to trigger execution
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- Simple junction table tracking which trades went into which executions
CREATE TABLE execution_trades (
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  executed_amount REAL NOT NULL,  -- How much of the trade was executed
  PRIMARY KEY (schwab_execution_id, onchain_trade_id)
);
```

**Example Flow for Unified Fractional Share Handling:**
1. Onchain trade: 1.1 AAPLs1 → `TradeAccumulator::add_trade()`:
   - Updates net_position (+1.1), accumulated_short (+1.1)  
   - Checks threshold: 1.1 > 1.0 → triggers execution
   - Creates SchwabExecution(AAPL, 1 share, SELL)
   - Records 1.0 in execution_trades, leaves 0.1 in accumulated_short
   - Returns Some(execution)
2. Later trade: 0.8 AAPLs1 → `TradeAccumulator::add_trade()`:
   - Updates accumulated_short (+0.8 = 0.9 total)
   - Checks threshold: 0.9 < 1.0 → no execution
   - Returns None
3. Another trade: 0.2 AAPLs1 → `TradeAccumulator::add_trade()`:
   - Updates accumulated_short (+0.2 = 1.1 total)  
   - Checks threshold: 1.1 > 1.0 → triggers execution
   - Creates SchwabExecution(AAPL, 1 share, SELL)
   - Records trades in execution_trades, leaves 0.1 in accumulated_short
   - Returns Some(execution)

**Benefits of Unified Design:**
- ✅ **Single Domain Object**: All fractional logic in one `TradeAccumulator` class
- ✅ **Simple Interface**: One method `add_trade()` handles everything
- ✅ **No Coordinator Logic**: Domain object manages its own state and executions
- ✅ **Clear Database Schema**: One accumulator table + simple junction table
- ✅ **Easier Testing**: Test one object with clear inputs and outputs
- ✅ **Zero Cognitive Overhead**: Understand one concept, not three

## Task 3. Replace Old System with New Unified System ✅ COMPLETED

✅ **Core Achievement:** Successfully migrated main event processing logic to unified TradeAccumulator system

**What was completed:**
- [x] **Main Processing Logic Updated**: `lib.rs` now uses unified workflow:
  - Parse events → `OnchainTrade` → `TradeAccumulator::add_trade()` → Execute if triggered
  - Completely replaced old `ArbTrade`-based workflow in main event loop
  - Added `execute_pending_schwab_execution()` function for Schwab API integration
- [x] **Database Integration**: New system fully integrated with unified schema
- [x] **Test Coverage**: All 159 tests passing with unified approach
- [x] **Code Quality**: All clippy lints resolved, code formatted
- [x] **Background Execution**: Async Schwab order execution with proper error handling

**Key Architectural Achievement:**
The **core arbitrage bot functionality** now runs on the unified TradeAccumulator system. The main event processing loop that monitors blockchain events and executes Schwab trades is completely migrated.

**Note:** CLI still uses old ArbTrade system for manual operations, but this is acceptable since:
- CLI is non-critical (manual operations only, not part of main bot workflow)
- Core bot functionality (the primary business value) is fully migrated
- Old and new systems can coexist during transition period

## Task 4. Code Quality and Architecture Refactoring

**Strategy:** Address code quality issues and CLAUDE.md principle violations identified in comprehensive review to improve maintainability and reduce complexity.

### 4.1 Refactor TradeAccumulator God Object
**Problem:** Single 530+ line class handles business logic, database persistence, and execution triggering, violating single responsibility principle.

- [ ] **Extract Position Calculator**: Create separate `PositionCalculator` struct for threshold logic and position tracking
  - `should_execute_long()`, `should_execute_short()` methods
  - Position validation and threshold checking
- [ ] **Extract Database Repository**: Move all SQL operations to `TradeAccumulatorRepository`
  - `save_within_transaction()`, `get_or_create_within_transaction()`, `find_by_symbol()`
  - All database-specific logic separated from business rules
- [ ] **Extract Business Logic**: Keep only pure business logic in `TradeAccumulator`
  - Trade accumulation calculations
  - Execution decision logic without database coupling
- [ ] **Create Domain Services**: Separate orchestration logic from business rules
  - `TradeExecutionService` for coordinating between components
  - Clear interfaces between domain objects

### 4.2 Fix Deep Nesting and Control Flow
**Problem:** Complex nested logic in `try_execute_position` (lines 145-185) violates CLAUDE.md "avoid deep nesting" principle.

- [ ] **Flatten Nested Logic**: Replace nested match/if statements with early returns
- [ ] **Extract Helper Functions**: Break down complex methods into smaller, focused functions
- [ ] **Use Pattern Matching with Guards**: Replace nested conditionals with pattern matching
- [ ] **Apply Functional Programming**: Use iterator chains and map/filter operations where appropriate

### 4.3 Improve Error Handling Architecture
**Problem:** `TradeConversionError` mixes database, business logic, and external API errors creating confusing error types.

- [ ] **Create Domain-Specific Errors**: Separate error types by concern
  - `TradeValidationError` for business rule violations
  - `PersistenceError` for database operations
  - `ExecutionError` for Schwab API failures
- [ ] **Implement Error Mapping**: Clear boundaries between layers with proper error conversion
- [ ] **Add Error Context**: Use anyhow for error chaining where appropriate
- [ ] **Remove Error Conflation**: Stop using single error type across multiple domains

### 4.4 Address Number Type and Casting Issues
**Problem:** Extensive use of `#[allow(clippy::cast_precision_loss)]` and `#[allow(clippy::cast_possible_truncation)]` suggests design issues.

- [ ] **Review Number Type Choices**: Evaluate whether f64 is appropriate for financial calculations
- [ ] **Consider Decimal Types**: Investigate rust_decimal crate for exact precision
- [ ] **Fix Root Causes**: Address underlying issues instead of suppressing warnings
- [ ] **Document Precision Decisions**: Where precision loss is acceptable, document rationale

### 4.5 Clean Up Comments and Documentation
**Problem:** Many comments violate CLAUDE.md principles by restating obvious code instead of explaining business logic.

- [ ] **Remove Redundant Comments**: Eliminate comments that restate what code obviously does
  - "Save the trade as immutable fact" (line 54-55 trade_accumulator.rs)
  - "Get or create accumulator for this symbol" (obvious from method name)
- [ ] **Keep Business Logic Explanations**: Retain comments explaining complex domain rules
  - Fractional share accumulation logic explanations
  - Symbol suffix validation rationale
- [ ] **Update Method Documentation**: Focus on "why" rather than "what" in doc comments
- [ ] **Remove Obvious Test Comments**: Clean up test setup descriptions that add no value

### 4.6 Improve Testing Architecture
**Problem:** Tests mix unit testing with database integration and have extensive setup duplication.

- [ ] **Separate Unit from Integration Tests**: Clear boundaries between business logic and database tests
  - Pure unit tests for business logic (no database)
  - Integration tests for database operations
- [ ] **Create Test Builders**: Reduce test setup duplication with builder patterns
  - `OnchainTradeBuilder`, `SchwabExecutionBuilder` for test data
- [ ] **Mock External Dependencies**: Proper mocking for Schwab API interactions
  - Extract interfaces for testability
- [ ] **Simplify Test Database Setup**: Reusable test utilities for database initialization

### 4.7 Standardize Struct Field Access Patterns
**Problem:** Inconsistent field access patterns between direct access and getter-like complexity.

- [ ] **Review Field Access Consistency**: Ensure consistent approach across codebase
- [ ] **Simplify Database Conversion Logic**: Remove unnecessary complexity in `convert_rows_to_executions!` macro
- [ ] **Apply CLAUDE.md Field Access Guidelines**: Direct access for simple data, methods only when adding logic

**Acceptance Criteria:**
- [ ] All clippy warnings resolved without suppress directives
- [ ] Tests maintain current coverage (159+ tests passing)
- [ ] Code follows CLAUDE.md principles consistently
- [ ] Reduced complexity metrics (cyclomatic complexity, nesting depth)
- [ ] Clear separation of concerns between components
- [ ] Ensure test/clippy/fmt pass: `cargo test -q && cargo clippy -- -D clippy::all && cargo fmt`

**Benefits of This Refactoring:**
- ✅ **Maintainability**: Easier to understand and modify code with clear responsibilities
- ✅ **Testability**: Separated concerns enable better unit testing
- ✅ **Code Quality**: Adherence to established architectural principles
- ✅ **Reduced Complexity**: Flattened control flow and focused components
- ✅ **Better Error Handling**: Clear error types and proper error propagation
- ✅ **Consistency**: Uniform patterns throughout codebase

## Task 5. Schema Cleanup

**NOTE: We modify the existing migration file directly instead of creating new migrations for simplicity**

- [ ] Modify existing migration to remove old `trades` table definition and `trade_executions` table
- [ ] Remove old struct definitions (`ArbTrade`, `TradeExecutionLink`) and database methods
- [ ] Clean up any temporary configuration options
- [ ] Remove complex coordinator allocation logic, replace with simple batch operations
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

**Current State:** Task 3 (Core Functionality) has been completed! The main arbitrage bot now runs entirely on the unified TradeAccumulator architecture. All 159 tests pass.

**Major Achievement - Core System Migration:**
- ✅ **Main Event Processing**: `lib.rs` completely migrated from old `ArbTrade` system to unified `TradeAccumulator` workflow
- ✅ **Unified Architecture**: Single `TradeAccumulator::add_trade()` method handles ALL fractional share logic
- ✅ **Background Execution**: Schwab orders executed asynchronously with proper error handling
- ✅ **Full Integration**: Database, tests, and core processing logic all use unified system
- ✅ **Code Quality**: All clippy warnings resolved, code formatted, 159 tests passing

**Architecture Transformation (COMPLETE):**
- **Before**: Complex workflow with `ArbTrade` → `execute_trade()` → status tracking across multiple tables
- **After**: Simple workflow with `OnchainTrade` → `TradeAccumulator::add_trade()` → optional Schwab execution

**System Impact:**
The **core arbitrage functionality** - monitoring blockchain events and executing offsetting Schwab trades - now operates on the clean, unified architecture. This is the primary business value delivery mechanism.

**Next Steps:** 
- Task 4: Code Quality and Architecture Refactoring (address CLAUDE.md violations)
- Task 5: Schema cleanup (remove old `trades` table)  
- Task 6: Refactor TradeStatus enum
- CLI updates (non-critical - manual operations only)

---

## Schema Design (Target Batch-Centric Architecture)

The new batch-centric schema encapsulates fractional share logic in domain objects:

```sql
-- FINAL BATCH-CENTRIC SCHEMA for fractional share accumulation

-- Onchain trades remain immutable blockchain facts
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
  shares INTEGER NOT NULL,  -- Always whole numbers (Schwab API limitation)
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL,
  executed_at TIMESTAMP
);

-- Trade batches encapsulate accumulation logic and state
CREATE TABLE trade_batches (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  target_shares INTEGER NOT NULL,
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  current_amount REAL NOT NULL DEFAULT 0.0,
  status TEXT CHECK (status IN ('ACCUMULATING', 'READY', 'EXECUTING', 'COMPLETED', 'FAILED')) NOT NULL DEFAULT 'ACCUMULATING',
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  completed_at TIMESTAMP
);

-- Track which trades contribute to which batches
CREATE TABLE batch_trades (
  batch_id INTEGER REFERENCES trade_batches(id),
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  contributed_amount REAL NOT NULL,
  PRIMARY KEY (batch_id, onchain_trade_id)
);

-- Position tracking simplified for threshold checking only
CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);
```

**Key Design Principles:**
- **TradeBatch domain objects** encapsulate all fractional accumulation logic
- **Clear state machine** with explicit transitions (Accumulating → Ready → Executing → Completed)
- **Single conceptual model** - "trades accumulate in batches until executable"
- **Simplified coordinator** - just find/create batch, add trade, execute if ready

## Task 6. Refactor TradeStatus to Sophisticated Enum

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
