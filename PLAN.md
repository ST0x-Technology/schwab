# Alpaca Broker Integration Plan

This plan outlines the integration of Alpaca Markets as an additional broker
option for the st0x arbitrage bot, leveraging the existing broker abstraction
layer.

## Task 1: Add Alpaca Dependencies and Auth Configuration

Add the necessary dependencies and basic configuration structure for Alpaca
integration.

- [x] Add `apca` crate dependency to `crates/broker/Cargo.toml`
- [x] Add `chrono-tz` for timezone handling if not already present
- [x] Create `crates/broker/src/alpaca/mod.rs` as the main module entry point
- [x] Create `crates/broker/src/alpaca/auth.rs` with `AlpacaAuthEnv` struct
      containing:
  - `api_key_id: String`
  - `api_secret_key: String`
  - `base_url: String` (for paper vs live trading)
- [x] Update `crates/broker/src/lib.rs` to add `Alpaca` variant to
      `SupportedBroker` enum
- [x] Update the Display impl for `SupportedBroker` to handle Alpaca variant
- [x] Add `pub mod alpaca` declaration to `crates/broker/src/lib.rs`
- [x] Update database string parsing in `src/offchain/execution.rs` to handle
      "alpaca" broker type

## Task 2: Implement Core Alpaca Authentication

Implement the authentication layer for Alpaca API access using API keys.

- [x] Define `AlpacaClient` struct wrapping `apca::Client` in `auth.rs`
- [x] Implement `AlpacaClient::new()` that creates client from environment
- [x] Add paper trading detection based on base_url
- [x] Implement account verification endpoint call to validate credentials
- [x] Use existing `apca::Error` types with `#[from]` conversion (no custom
      AlpacaError enum needed)
- [x] Implement `From<apca::Error>` for `BrokerError` with boxed errors for
      performance
- [x] Add unit tests for auth configuration with valid/invalid API keys
- [x] Add test for paper vs live environment detection

## Task 3: Implement Market Hours Detection

Create market hours checking functionality using Alpaca's Clock API.

- [x] Create `crates/broker/src/alpaca/market_hours.rs`
- [x] Implement `wait_until_market_open()` function using Alpaca Clock API
- [x] Add timezone conversion from Eastern to UTC
- [x] Implement logic to determine if market is currently open
- [x] Calculate wait duration until next market open
- [x] Handle weekends and market holidays (delegated to Alpaca API)
- [x] Write tests with mocked clock API responses
- [x] Test edge cases: open market, closed market, future dates

## Task 4: Implement Order Placement

Map the generic order types to Alpaca's order API.

- [x] Create `crates/broker/src/alpaca/order.rs`
- [x] Define Alpaca order request structure matching their API
- [x] Implement conversion from `MarketOrder` to Alpaca order request
- [x] Map `Direction` enum to Alpaca's buy/sell instructions
- [x] Implement order submission via apca crate
- [x] Parse order response to extract order ID
- [x] Convert response to `OrderPlacement` struct
- [x] Handle Alpaca-specific validations (day trading restrictions, etc.)
- [x] Map Alpaca error responses to `BrokerError` variants
- [x] Add tests for successful and failed order placements

## Task 5: Implement Order Status Polling

Implement order status checking and batch polling functionality.

- [x] Write comprehensive tests for order status functionality
- [x] Complete actual API implementation to replace placeholder
      `get_order_status()` function
- [x] Define order status query functions using apca with proper type handling
- [x] Map Alpaca order statuses to `OrderStatus` enum:
  - `new`/`accepted`/`pending_new` → `Submitted`
  - `filled` → `Filled`
  - `rejected`/`canceled`/`expired` → `Failed`
  - `partially_filled` → handle appropriately
- [x] Implement proper `get_order_status()` with actual Alpaca API calls
- [x] Implement `poll_pending_orders()` for batch status checks
- [x] Extract execution price from filled orders
- [x] Handle partial fills with appropriate state
- [x] Add retry logic for transient failures
- [x] Replace placeholder implementation with real API integration

**Current Status**: ✅ **COMPLETED** - Full API integration implemented with
comprehensive test coverage. Both `get_order_status()` and
`poll_pending_orders()` functions are fully functional with proper error
handling, status mapping, and price extraction.

## Task 6: Create AlpacaBroker Implementation

Implement the complete Broker trait for Alpaca.

- [x] Create `crates/broker/src/alpaca/broker.rs`
- [x] Define `AlpacaBroker` struct with client and configuration
- [x] Define `AlpacaConfig` type alias for configuration tuple
- [x] Implement `try_from_config()` with credential validation
- [x] Implement `wait_until_market_open()` using market hours module
- [x] Implement `place_market_order()` using order module
- [x] Implement `get_order_status()` using status polling
- [x] Implement `poll_pending_orders()` for batch updates
- [x] Implement `parse_order_id()` for database compatibility
- [x] Implement `to_supported_broker()` returning `SupportedBroker::Alpaca`
- [x] Implement `run_broker_maintenance()` (return None - no token refresh
      needed)
- [x] Add comprehensive integration tests

**Status**: ✅ **COMPLETED** - Full AlpacaBroker implementation with
comprehensive test coverage. All trait methods implemented, delegating to
existing alpaca modules. Dead code warnings resolved.

## Task 7: Update Main Application Integration

Integrate AlpacaBroker into the main application's broker selection logic.

- [x] Add `AlpacaAuthEnv` to imports in `src/env.rs`
- [x] Add `alpaca_auth: Option<AlpacaAuthEnv>` to main `Env` struct
- [x] Add `--broker` CLI argument with choices: schwab, alpaca, dry-run
  - Created `BrokerChoice` enum with `Schwab`, `Alpaca`, `DryRun` variants
  - Replaced `dry_run: bool` field with `broker: BrokerChoice`
- [x] Implement `get_alpaca_broker()` helper method in `Env`
- [x] Update `run_bot_session()` to support three-way broker selection
  - Uses pattern matching on `env.broker` to select appropriate broker
- [x] Update CLI test command to support Alpaca broker
  - Modified `process_found_trade()` to handle all three broker types
  - Updated authentication flow for each broker type
- [x] Ensure database properly stores broker='alpaca' for Alpaca trades
  - Database parsing already supports "alpaca" broker type (from Task 1)
- [x] Update any hardcoded `SupportedBroker::Schwab` references
  - Updated in `src/cli.rs` to use dynamic `broker_type` based on env.broker
- [x] Implement `Clone` for `AlpacaBroker` and `AlpacaClient`
  - Added credential fields to `AlpacaClient` to enable cloning
  - Implemented manual `Clone` trait since `apca::Client` doesn't implement
    Clone
- [x] Update all test helper functions with new `Env` struct fields
  - Updated `create_test_env_for_cli()` in src/cli.rs
  - Updated `create_test_env()` in src/api.rs and src/env.rs
- [x] Add error handling for missing Alpaca credentials
  - Created `BrokerConfigError::MissingAlpacaCredentials`
  - Implemented conversion to `BrokerError::Authentication`
- [ ] Test broker selection logic with all three options
- [ ] Verify database correctly tracks different broker trades

**Status**: ✅ **MOSTLY COMPLETED** - All code implementation finished. The
application now supports three-way broker selection via the `--broker` CLI
argument. Project compiles successfully with `cargo check` and `cargo fmt`
passes. Integration testing with all three broker options still needs to be
performed.

## Task 8: Finishing touches

- [ ] Add rate limit handling with exponential backoff (using backon) (iff missing)
- [ ] Write unit tests for all new Alpaca modules
- [ ] Create tests using `httpmock` for Alpaca API
- [ ] Add test coverage for error scenarios
- [ ] Add tests seemlessly broker switching between Schwab and Alpaca
- [ ] Document Alpaca account setup process in @README (be brief)
- [ ] Update architecture documentation @SPEC.md to include Alpaca
- [ ] Verify all checks pass
  - [ ] All tests
  - [ ] All clippy rules
  - [ ] All formatting rules
  - [ ] Issues are actually resolved, not just errors/warnings bypassed
