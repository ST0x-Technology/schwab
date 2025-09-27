# Broker Abstraction Refactoring Plan

## Overview

This plan refactors the arbitrage bot to support multiple brokers through a
trait-based abstraction. The current implementation is coupled to Charles
Schwab. We'll create a workspace with the main bot remaining at the root and
extract only the Schwab broker implementation into a separate crate, using a
clean broker interface with generics for polymorphism.

## Task 1: Setup Workspace Structure

- [x] Backup current `Cargo.toml`
- [x] Create workspace `Cargo.toml` at root with members
      `[".", "crates/broker"]` and resolver `"2"`
- [x] Update main crate name from `rain_schwab` to `st0x-arbot`
- [x] Fix all source code references from `rain_schwab` to `st0x_arbot`
- [x] Create `crates/broker/` directory structure
- [x] Create `crates/broker/Cargo.toml` with name `st0x-broker`
- [x] Set up workspace dependencies for shared crates
- [x] Add broker crate as dependency in main crate

## Task 2: Define Broker Interface

- [x] Create complete broker trait in `crates/broker/src/lib.rs` with all domain
      types
- [x] Define `Broker` trait with associated types `Error` and `OrderId`
- [x] Add trait methods: `ensure_ready`, `place_market_order`,
      `get_order_status`, `poll_pending_orders`
- [x] Define domain newtypes: `Symbol`, `Shares`, `Direction`, `MarketOrder`
- [x] Define order lifecycle types: `OrderState` (with enum variants in separate
      module), `OrderPlacement`, `OrderUpdate`
- [x] Define broker-agnostic `BrokerError` enum
- [x] Add MockBroker implementation for testing
- [x] Ensure everything compiles and tests pass

## Task 3: Refactor Core Bot Logic

- [x] Update `src/conductor.rs` to use generic `Conductor<B: Broker>`
- [x] Refactor conductor methods to work with broker trait
- [x] Update `run_live` function to accept broker parameter
- [x] Add MockBroker temporarily for compilation
- [x] Ensure all tests compile and pass

## Task 4: Reconcile Conflicting Broker Traits and Remove Duplicates (COMPLETED)

After rebasing on the dry-run feature branch, we have two conflicting broker
traits and duplicate types that need to be reconciled:

- New trait: `crates/broker/src/lib.rs` (the abstraction we're building)
- Old trait: `src/schwab/broker.rs` (added for dry-run mode with `Schwab` and
  `LogBroker`)
- Duplicate types: `Direction` enum exists in both `src/schwab/mod.rs` and
  `crates/broker/src/lib.rs`
- `SchwabExecution` is not broker-agnostic and should be renamed

- [x] Remove the old broker trait from `src/schwab/broker.rs`
- [x] Port `LogBroker` dry-run implementation to new trait as `DryRunBroker` in
      `crates/broker/`
- [x] Update `src/env.rs` to use the new broker trait from `st0x_broker` crate
- [x] Remove `DynBroker` type alias and use generics instead of trait objects
- [x] Fix import conflict in `src/conductor.rs` line 21 (both traits imported)
- [x] Update all conductor functions to use generic `B: Broker` parameter
- [x] Migrate existing `Schwab` implementation to implement the new `Broker`
      trait
- [x] Remove duplicate `Direction` enum from `src/schwab/mod.rs`
- [x] Create `SupportedBroker` enum in broker crate (Schwab, DryRun)
- [x] Add `to_supported_broker()` method to Broker trait
- [x] Rename `SchwabExecution` → `OffchainExecution`
- [x] Add `broker: SupportedBroker` field to `OffchainExecution`
- [x] Update all imports to use `st0x_broker::Direction`
- [x] Update `env.rs` to return concrete broker types
- [x] Unify SchwabAuthEnv types (delete duplicate)
- [x] Fix method visibility issues
- [x] Consolidate PersistenceError types
- [x] Enhance broker trait to return OrderState from get_order_status
- [x] Remove unnecessary config parameter from broker trait methods
- [x] Create unified TestBroker (merge MockBroker and DryRunBroker)
- [x] Add price_cents to OrderUpdate struct

## Task 5: Implement Schwab Broker

- [x] Move `src/schwab/` code to `crates/broker/src/schwab/`
- [x] Add required dependencies to broker crate
- [x] Update schwab module references and make it compile
- [x] Implement `Broker` trait for `SchwabBroker`
- [x] Update main application to use `SchwabBroker` instead of `MockBroker`
- [x] Adapt existing Schwab order placement logic to trait methods
- [x] Keep Schwab OAuth and token refresh as broker-specific methods
- [x] Ensure Schwab implementation compiles and works

## Task 6: Database Migration

- [x] Create migration script to rename `schwab_executions` to `offchain_trades`
      (improved naming for consistency with `onchain_trades`)
- [x] Add `broker_type` column with default 'schwab'
- [x] Add `broker_order_id` column for generic order tracking
- [x] Update all SQL queries in broker crate (15+ references)
- [x] Update all SQL queries in main crate
- [x] Update foreign key references in `trade_accumulators` table
- [x] Update foreign key references in `trade_execution_links` table
- [x] Update indexes to use new table name
- [x] Test migration runs successfully and schema is correct
- [x] Verify SQLX compile-time verification passes
- [x] Fix compilation issues with new schema
- [ ] Fix failing tests related to database constraints
- [ ] Remove duplicate schwab execution module from main crate (deferred)

## Task 7: Complete src/schwab/ Directory Removal (COMPLETED)

### Recent Progress (Completed):
- [x] Fixed HasOrderStatus trait organization (moved from order/state.rs to order/mod.rs)
- [x] Replaced all TradeState/TradeStatus references with OrderState/OrderStatus from broker crate
- [x] Fixed import organization - moved function-level imports to module level in tests
- [x] Fixed missing imports and visibility issues throughout codebase
- [x] Cleaned up Schwab-specific imports to use proper broker crate paths
- [x] Fixed duplicate module references and removed conflicting definitions

### Current Status:
The `src/schwab/` directory still exists with mixed generic and Schwab-specific
code. Need to properly move all code and update the launch function to use the
correct broker based on the --dry-run flag.

### Analysis of Remaining Files:

- **execution.rs** - Generic offchain execution handling (move to main crate)
- **order_poller.rs** - Generic order polling that already uses Broker trait
  (move to main crate)
- **market_hours.rs** - Schwab-specific API calls (move to broker crate)
- **market_hours_cache.rs** - Schwab-specific caching (move to broker crate)
- **order.rs** - Schwab-specific order placement (move to broker crate)
- **order_status.rs** - Schwab-specific order status checking (move to broker
  crate)
- **shares_from_db_i64 utility** in mod.rs - Generic database utility (move to
  main crate)

**IMPORTANT**: There is NO TradeStatus/TradeState enum to move. The main crate
should use `OrderState` and `OrderStatus` from the broker crate (`st0x_broker`).
All references to TradeState/TradeStatus should be replaced with OrderState/OrderStatus.

### Required Tasks (COMPLETED):

- [x] Create new modules in main crate for generic code:
  - `src/offchain/execution.rs` (from execution.rs, using OrderState from broker)
  - `src/offchain/order_poller.rs` (from order_poller.rs)
  - `src/db_utils.rs` (from shares_from_db_i64 utility)
- [x] Update all references to TradeState/TradeStatus to use OrderState/OrderStatus
  from `st0x_broker`
- [x] Move Schwab-specific code to broker crate:
  - `crates/broker/src/schwab/market_hours.rs`
  - `crates/broker/src/schwab/market_hours_cache.rs`
  - `crates/broker/src/schwab/order.rs` (complete migration)
  - `crates/broker/src/schwab/order_status.rs`
- [x] Update launch() function to select broker based on --dry-run flag
- [x] Handle Schwab-specific services conditionally:
  - Token refresh task: Only spawn for Schwab broker
  - Trading hours controller: Make broker-agnostic or Schwab-conditional
- [x] Update all imports throughout codebase to use new locations
- [x] Move OAuth flow from src/schwab/mod.rs to src/cli.rs
- [x] Delete entire src/schwab/ directory
- [x] Add chrono-tz dependency to broker crate for market hours functionality
- [x] Run compilation tests to ensure core functionality works

## Task 7b: Fix Broker Crate Boundary Violations (COMPLETED)

**CRITICAL ARCHITECTURAL ISSUE:** The broker crate incorrectly contains
`OnChainError` and other on-chain concerns, violating the clean separation of
concerns. The broker crate should be a pure broker abstraction with no knowledge
of blockchain/on-chain concepts.

### Boundary Violations Found:

1. **OnChainError in broker crate**: Defined in `crates/broker/src/error.rs` and
   exported from `crates/broker/src/lib.rs`
2. **Duplicated execution modules**: Both `src/schwab/execution.rs` and
   `crates/broker/src/schwab/execution.rs` exist
3. **Mixed error concerns**: Broker crate's `SchwabExecution` depends on
   `OnChainError`
4. **Cross-boundary conversions**: `BrokerError` to `OnChainError` conversions
   in broker crate

### Files Violating Boundaries:

- `crates/broker/src/error.rs` - Contains `OnChainError` (lines 22-51)
- `crates/broker/src/lib.rs` - Exports `OnChainError`
- `crates/broker/src/schwab/execution.rs` - Uses `OnChainError` (duplicate
  module)
- `crates/broker/src/schwab/mod.rs` - Helper functions return `OnChainError`
- `crates/broker/src/schwab/order_status.rs` - `price_in_cents()` returns
  `OnChainError`

### Required Fixes:

- [x] Consolidate OrderState and TradeState into single OrderState (completed)
- [x] Remove `OnChainError` completely from broker crate
- [x] Delete duplicate `crates/broker/src/schwab/execution.rs`
- [x] Fix broker crate functions to return `BrokerError` or `PersistenceError`
- [x] Remove all `OnChainError` imports from broker crate
- [x] Remove cross-boundary error conversions
- [x] Verify broker crate only contains: `BrokerError`, `PersistenceError`
- [x] Verify main crate keeps: `OnChainError`, `TradeValidationError`, etc.

**Note**: Broker crate boundary violations have been successfully fixed.
However, compilation errors remain due to missing functions (`SchwabExecution`,
`update_execution_status_within_transaction`,
`find_executions_by_symbol_and_status`, `find_execution_by_id`) that need to be
properly moved from main crate to broker crate while maintaining clean
boundaries. This requires careful separation of execution logic to ensure broker
crate remains free of blockchain concerns.

### Post-Fix Architecture:

**Broker Crate (`st0x-broker`):**

- Pure broker abstraction layer
- No knowledge of blockchain/on-chain concepts
- Error types: `BrokerError`, `PersistenceError` only
- Can be used by any application needing broker access

**Main Crate (`st0x-arbot`):**

- Orchestrates on-chain and off-chain operations
- Bridges blockchain events to broker actions
- Error types: `OnChainError`, `TradeValidationError`, `EventProcessingError`
- Contains all blockchain-specific logic

## Task 8: Update Main Application (COMPLETED)

- [x] Handle Schwab-specific background tasks (token refresh) with runtime dry run mode
      checking
- [x] Update imports to use broker crate types
- [x] Ensure main application works with both Schwab and mock brokers

### Implementation Summary:

**IMPLEMENTATION APPROACH:**
- **Key Principle**: Only validate and refresh Schwab tokens when actually using Schwab broker
- **Broker-Conditional Logic**: Token validation occurs only in Schwab mode, not in dry-run mode with TestBroker

**1. Conditional Token Refresh (FINAL):**
- **Token Validation**: Only occurs when running with SchwabBroker (in else branch)
- **Token Refresh Background Task**: Made optional in `BackgroundTasks.token_refresher` 
- **Broker Selection**: Made conditional based on dry-run mode
- **TestBroker Mode**: Bypasses all Schwab-specific authentication (no token validation needed)
- Added broker type check in `BackgroundTasksBuilder.spawn()` to only spawn token refresh for Schwab brokers
- Fixed imports to use `st0x_broker::schwab::tokens` module

**2. Conditional Trading Hours Control:**
- Added dry-run mode bypass for trading hours control in `src/lib.rs`
- Trading hours controller only initialized and used for Schwab mode
- Dry-run mode runs immediately without market hours restrictions

**3. Import Updates:**
- Updated all `schwab::tokens::` references to use `st0x_broker::schwab::tokens`
- Fixed module references for moved components (OrderStatusPoller, OffchainExecution)
- Added proper broker trait imports where needed

**4. Broker-Specific Logic:**
- TestBroker used for dry-run mode (no token refresh, no trading hours)
- SchwabBroker used for production mode (with token refresh and trading hours)
- Both brokers implement the same `Broker` trait interface

**Files Modified:**
- `src/lib.rs` - Corrected token validation flow and conditional broker selection
- `src/conductor.rs` - Optional token refresher in background tasks
- `src/env.rs` - Fixed broker constructor calls
- `src/cli.rs` - Updated token imports 
- `src/schwab/mod.rs` - Removed missing module references, made imports public
- Test files - Updated token import paths

**Final Architecture:**
The main application now properly handles both Schwab and mock brokers with appropriate conditional logic for broker-specific services:
- **Dry-run mode (TestBroker)**: Bypasses token validation, token refresh, and trading hours control entirely
- **Production mode (SchwabBroker)**: Validates tokens, spawns token refresh background task, and uses trading hours control
- Both modes implement the same `Broker` trait interface for consistent orchestration logic

## Task 8b: Reduce Nesting in lib.rs and Extend Broker Abstraction

**Problem Statement:**
- The `run` function in `src/lib.rs` has 5 levels of deep nesting, violating CLAUDE.md guidelines
- Redundant `new` + `ensure_ready` pattern in broker trait
- Market hours control logic should be part of the broker abstraction
- Bot should restart on all errors without losing functionality

**Solution Approach:**
1. **Replace `new` + `ensure_ready` with `try_from_config`**: Single async initialization point that handles all validation
2. **Simplify market hours API**: Use single `wait_until_market_open()` method that returns `Option<Duration>`
3. **Flatten `src/lib.rs`**: Extract helper functions, eliminate deep nesting
4. **Unified code path**: Only one conditional for broker creation, everything else identical

**Key Changes:**

### 1. Update Broker Trait
- Remove `ensure_ready()` method entirely
- Replace `new()` with `async try_from_config()` that does all validation
- Add `wait_until_market_open() -> Option<Duration>` for market hours control
- All brokers implement the same interface

### 2. Update Broker Implementations
- **SchwabBroker**: `try_from_config` validates tokens, market hours uses real API
- **TestBroker**: `try_from_config` always succeeds, market hours returns None (always open)

### 3. Simplify lib.rs Structure
- Extract `initialize_event_streams()` helper function
- Single `run_bot_session()` with minimal nesting
- Main `run()` function just handles restart loop
- No conditional logic after broker creation

### Task Checklist:
- [x] Update Broker trait: remove ensure_ready, add try_from_config, simplify market methods
- [x] Update SchwabBroker implementation with try_from_config and market hours
- [x] Update TestBroker implementation with try_from_config
- [x] Update env.rs methods to use async try_from_config
- [x] Simplify src/lib.rs with helper functions and unified code path
- [x] Preserve background token refresh logic for Schwab broker during refactoring
- [x] Test for regressions in bot functionality (fixed market hours logic preservation)
- [x] Verify maximum 2-3 levels of nesting (down from 5)

**Benefits:**
- **Cleaner API**: Single initialization point, no redundant methods
- **Maximum 2-3 levels of nesting** (down from 5 levels)
- **Unified code path**: One conditional for broker creation
- **No regression**: All original functionality preserved
- **Resilient**: Bot restarts on any error
- **Follows CLAUDE.md**: Flat code, extracted functions, single responsibility

## Task 9: Update CLI and Testing

- [ ] Keep existing Schwab auth command
- [ ] Add test mode command to run with mock broker
- [ ] Update all existing tests to work with new structure
- [ ] Replace Schwab-specific mocks with MockBroker where appropriate
- [ ] Add unit tests for broker trait with MockBroker
- [ ] Add integration tests for both Schwab and mock brokers
- [ ] Ensure all tests pass

## Task 10: Documentation and Cleanup

- [ ] Update CLAUDE.md with new workspace structure
- [ ] Document broker abstraction architecture in crates/broker/CLAUDE.md
- [ ] Update development commands
- [ ] Add section on extending with new brokers
- [ ] Update README with new architecture
- [ ] Add doc comments to public broker APIs
- [ ] Clean up any remaining references to old structure

## Validation Checklist

- [ ] All existing tests pass
- [ ] Schwab functionality unchanged
- [ ] Mock broker works in tests
- [ ] Database migration is reversible
- [ ] No performance degradation
- [ ] Code follows CLAUDE.md guidelines
- [ ] Clean separation between core and broker code

## Future Extensibility

Adding a new broker will require:

1. Create new broker implementation in `crates/broker/src/newbroker/`
2. Implement `Broker` trait from broker crate
3. Handle broker-specific authentication separately
4. No changes to core bot logic in root crate

This design ensures:

- Type safety through generics
- Zero-cost abstraction (no dynamic dispatch in hot path)
- Clean separation of concerns with broker trait in separate crate
- Easy testing with mock broker
- Extensibility for future brokers like Alpaca
