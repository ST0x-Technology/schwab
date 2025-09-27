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
- [ ] Adapt existing Schwab order placement logic to trait methods
- [ ] Keep Schwab OAuth and token refresh as broker-specific methods
- [ ] Ensure Schwab implementation compiles and works

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

## Task 7: Complete src/schwab/ Directory Removal (CURRENT)

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
- **TradeStatus enum** in mod.rs - Generic status enum (move to main crate)
- **shares_from_db_i64 utility** in mod.rs - Generic database utility (move to
  main crate)

### Required Tasks:

- [ ] Create new modules in main crate for generic code:
  - `src/offchain_execution.rs` (from execution.rs)
  - `src/order_poller.rs` (from order_poller.rs)
  - `src/trade_status.rs` (from TradeStatus enum)
  - `src/db_utils.rs` (from shares_from_db_i64 utility)
- [ ] Move Schwab-specific code to broker crate:
  - `crates/broker/src/schwab/market_hours.rs`
  - `crates/broker/src/schwab/market_hours_cache.rs`
  - `crates/broker/src/schwab/order.rs` (complete migration)
  - `crates/broker/src/schwab/order_status.rs`
- [ ] Update launch() function to select broker based on --dry-run flag
- [ ] Handle Schwab-specific services conditionally:
  - Token refresh task: Only spawn for Schwab broker
  - Trading hours controller: Make broker-agnostic or Schwab-conditional
- [ ] Update all imports throughout codebase to use new locations
- [ ] Test dry-run mode works with --dry-run flag using DryRunBroker
- [ ] Test Schwab mode works correctly with SchwabBroker
- [ ] Delete entire src/schwab/ directory
- [ ] Run tests to ensure nothing breaks

## Task 8: Update Main Application

- [ ] Handle Schwab-specific background tasks (token refresh) with runtime type
      checking
- [ ] Update imports to use broker crate types
- [ ] Ensure main application works with both Schwab and mock brokers

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
- [ ] Document broker abstraction architecture
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
