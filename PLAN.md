# Alpaca Broker Integration Plan

This plan outlines the integration of Alpaca Markets as an additional broker option for the st0x arbitrage bot, leveraging the existing broker abstraction layer.

## Task 1: Add Alpaca Dependencies and Auth Configuration

Add the necessary dependencies and basic configuration structure for Alpaca integration.

- [x] Add `apca` crate dependency to `crates/broker/Cargo.toml`
- [x] Add `chrono-tz` for timezone handling if not already present
- [x] Create `crates/broker/src/alpaca/mod.rs` as the main module entry point
- [x] Create `crates/broker/src/alpaca/auth.rs` with `AlpacaAuthEnv` struct containing:
  - `api_key_id: String` 
  - `api_secret_key: String`
  - `base_url: String` (for paper vs live trading)
- [x] Update `crates/broker/src/lib.rs` to add `Alpaca` variant to `SupportedBroker` enum
- [x] Update the Display impl for `SupportedBroker` to handle Alpaca variant
- [x] Add `pub mod alpaca` declaration to `crates/broker/src/lib.rs`
- [x] Update database string parsing in `src/offchain/execution.rs` to handle "alpaca" broker type

## Task 2: Implement Core Alpaca Authentication

Implement the authentication layer for Alpaca API access using API keys.

- [x] Define `AlpacaClient` struct wrapping `apca::Client` in `auth.rs`
- [x] Implement `AlpacaClient::new()` that creates client from environment
- [x] Add paper trading detection based on base_url
- [x] Implement account verification endpoint call to validate credentials
- [x] Use existing `apca::Error` types with `#[from]` conversion (no custom AlpacaError enum needed)
- [x] Implement `From<apca::Error>` for `BrokerError` with boxed errors for performance
- [x] Add unit tests for auth configuration with valid/invalid API keys
- [x] Add test for paper vs live environment detection

## Task 3: Implement Market Hours Detection

Create market hours checking functionality using Alpaca's calendar API.

- [ ] Create `crates/broker/src/alpaca/market_hours.rs`
- [ ] Define `MarketHours` struct to hold market schedule data
- [ ] Implement `fetch_market_calendar()` to get current day's schedule
- [ ] Add timezone conversion from Eastern to UTC
- [ ] Implement logic to determine if market is currently open
- [ ] Calculate wait duration until next market open
- [ ] Handle weekends and market holidays
- [ ] Add support for extended hours configuration
- [ ] Write tests with mocked calendar API responses
- [ ] Test edge cases: holidays, weekends, after-hours

## Task 4: Implement Order Placement

Map the generic order types to Alpaca's order API.

- [ ] Create `crates/broker/src/alpaca/order.rs`
- [ ] Define Alpaca order request structure matching their API
- [ ] Implement conversion from `MarketOrder` to Alpaca order request
- [ ] Map `Direction` enum to Alpaca's buy/sell instructions
- [ ] Implement order submission via apca crate
- [ ] Parse order response to extract order ID
- [ ] Convert response to `OrderPlacement` struct
- [ ] Handle Alpaca-specific validations (day trading restrictions, etc.)
- [ ] Map Alpaca error responses to `BrokerError` variants
- [ ] Add tests for successful and failed order placements

## Task 5: Implement Order Status Polling

Implement order status checking and batch polling functionality.

- [ ] Define order status query functions using apca
- [ ] Map Alpaca order statuses to `OrderState` enum:
  - `new`/`accepted`/`pending_new` → `Submitted`
  - `filled` → `Filled`
  - `rejected`/`canceled`/`expired` → `Failed`
  - `partially_filled` → handle appropriately
- [ ] Implement `get_order_status()` for single order queries
- [ ] Implement `poll_pending_orders()` for batch status checks
- [ ] Extract execution price from filled orders
- [ ] Handle partial fills with appropriate state
- [ ] Add retry logic for transient failures
- [ ] Write tests for all order state transitions
- [ ] Test error scenarios and edge cases

## Task 6: Create AlpacaBroker Implementation

Implement the complete Broker trait for Alpaca.

- [ ] Create `crates/broker/src/alpaca/broker.rs`
- [ ] Define `AlpacaBroker` struct with client and configuration
- [ ] Define `AlpacaConfig` type alias for configuration tuple
- [ ] Implement `try_from_config()` with credential validation
- [ ] Implement `wait_until_market_open()` using market hours module
- [ ] Implement `place_market_order()` using order module
- [ ] Implement `get_order_status()` using status polling
- [ ] Implement `poll_pending_orders()` for batch updates
- [ ] Implement `parse_order_id()` for database compatibility
- [ ] Implement `to_supported_broker()` returning `SupportedBroker::Alpaca`
- [ ] Implement `run_broker_maintenance()` (return None - no token refresh needed)
- [ ] Add comprehensive integration tests

## Task 7: Update Main Application Integration

Integrate AlpacaBroker into the main application's broker selection logic.

- [ ] Add `AlpacaAuthEnv` to imports in `src/env.rs`
- [ ] Add `alpaca_auth: Option<AlpacaAuthEnv>` to main `Env` struct
- [ ] Add `--broker` CLI argument with choices: schwab, alpaca, dry-run
- [ ] Implement `get_alpaca_broker()` helper method in `Env`
- [ ] Update `run_bot_session()` to support three-way broker selection
- [ ] Update CLI test command to support Alpaca broker
- [ ] Ensure database properly stores broker='alpaca' for Alpaca trades
- [ ] Update any hardcoded `SupportedBroker::Schwab` references
- [ ] Test broker selection logic with all three options
- [ ] Verify database correctly tracks different broker trades

## Task 8: Add Alpaca-Specific Features

Implement features unique to Alpaca that enhance the bot's capabilities.

- [ ] Add configuration for fractional share trading support
- [ ] Implement extended hours trading flag in order placement
- [ ] Add rate limit handling with exponential backoff
- [ ] Implement Alpaca-specific error recovery strategies
- [ ] Add configuration for paper trading mode
- [ ] Handle pattern day trader restrictions if applicable
- [ ] Add support for querying buying power/account status
- [ ] Implement position querying for reconciliation
- [ ] Document all Alpaca-specific configuration options
- [ ] Add example .env configuration for Alpaca

## Task 9: Testing and Documentation

Comprehensive testing and documentation for the Alpaca integration.

- [ ] Write unit tests for all new Alpaca modules
- [ ] Create integration tests using `httpmock` for Alpaca API
- [ ] Test full order lifecycle in paper trading environment
- [ ] Add test coverage for error scenarios
- [ ] Test broker switching between Schwab and Alpaca
- [ ] Document Alpaca account setup process in README
- [ ] Add Alpaca configuration section to CLAUDE.md
- [ ] Create comparison table of Schwab vs Alpaca features
- [ ] Document how to obtain Alpaca API keys
- [ ] Add troubleshooting guide for common Alpaca issues
- [ ] Update architecture documentation to include Alpaca
- [ ] Verify all tests pass with `cargo test -p st0x-broker`