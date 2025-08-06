# Implementation Plan: CLI Extension for Opposite-Side Trade Testing

Based on analysis of the existing codebase, this plan extends the current CLI to test the ability to take the opposite side of trades given a transaction hash. The implementation will maximize code reuse by leveraging existing trade logic components.

## Task 1. Create Transaction Hash to Trade Converter ✅

Create new functionality to reconstruct trades from transaction hashes, leveraging existing `PartialArbTrade` constructors:

**FIXES COMPLETED:**
- [x] Fix warning logic: only emit warning when multiple trades found, not on every success
- [x] Refactor to use functional programming patterns (iterator chains instead of imperative loops)
- [x] Add comprehensive test coverage for all scenarios
- [x] Reduce nesting by extracting helper functions

**Implementation Tasks:**
- [x] Create @src/trade/processor.rs module to house transaction hash processing logic
- [x] Add `try_from_tx_hash` function that takes `tx_hash`, `provider`, `env` and returns `Result<PartialArbTrade, TradeConversionError>` (extend the error type in case no trade is found for tx hash)
- [x] **FIXED**: Implement transaction receipt lookup using functional patterns (filter_map, find_map)
- [x] **FIXED**: Extract log processing into separate functions to reduce nesting
- [x] **FIXED**: Only warn when multiple valid trades are found, not on every success  
- [x] **FIXED**: Use iterator chains instead of imperative for loops
- [x] Filter logs by orderbook contract address and use existing constructors
- [x] **ADDED**: Test cases for ClearV2 events with successful trade conversion
- [x] **ADDED**: Test cases for orderbook events that don't match target order
- [x] Handle edge cases: transaction not found, no relevant logs
- [x] Ensure tests pass: `cargo test` ✅ (4 comprehensive tests)
- [x] Ensure clippy passes: `cargo clippy` ✅ (no warnings/errors)
- [x] Ensure fmt passes: `cargo fmt` ✅ (properly formatted)
- [x] Update TODOs.md with completion status

## Task 2. Extend CLI Commands with Transaction Hash Processing

Add new CLI command that processes a transaction hash to create and execute opposite-side trades:

- [ ] Add new `ProcessTx` variant to `Commands` enum in `@src/cli.rs`
- [ ] Add tx_hash parameter, RPC URL parameter, and optional block number parameter to CLI args
- [ ] Create `process_tx_command` function that validates transaction hash format
- [ ] Implement transaction lookup using existing `EvmEnv` and provider setup patterns
- [ ] Add comprehensive validation for transaction hash format (0x prefixed, 64 hex chars)
- [ ] Update `@src/cli.rs` command matching to handle new `ProcessTx` variant
- [ ] Add integration tests for new command in CLI test module
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

## Task 3. Create Transaction Hash to Trade Conversion Logic

Implement the core logic to convert a transaction hash into a tradeable `PartialArbTrade`:

- [ ] Add `try_from_transaction_hash` function to `@src/trade/processor.rs`
- [ ] Implement transaction receipt lookup using alloy provider
- [ ] Parse transaction logs for `ClearV2` and `TakeOrderV2` events from the orderbook
- [ ] Filter logs by orderbook contract address from `EvmEnv`
- [ ] Use existing `PartialArbTrade::try_from_clear_v2` and `try_from_take_order_if_target_order` methods
- [ ] Handle case where transaction contains multiple relevant logs (return Vec or first match)
- [ ] Add comprehensive error handling for invalid transaction hash, network issues, no relevant events
- [ ] Add unit tests with mocked provider responses
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

## Task 4. Integrate Trade Execution with Existing Schwab Logic

Connect the new transaction processing with existing Schwab order execution:

- [ ] Modify `execute_order_with_writers` in `@src/cli.rs` to accept `PartialArbTrade` instead of raw ticker/quantity
- [ ] Create `execute_trade_from_partial` function that converts `PartialArbTrade` to Schwab order
- [ ] Reuse existing authentication, order validation, and execution logic from current CLI
- [ ] Add proper error propagation from trade conversion to CLI output
- [ ] Ensure database integration works (save `ArbTrade` to database like main bot)
- [ ] Add success/failure output messages showing onchain vs offchain trade details
- [ ] Test end-to-end flow with mock Schwab API responses
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

## Task 5. Add Comprehensive CLI Help and Validation

Ensure the new command has proper help text, validation, and error handling:

- [ ] Add comprehensive help text for new `process-tx` command explaining parameters
- [ ] Add examples in help text showing proper usage with sample transaction hashes
- [ ] Implement robust input validation for transaction hash, RPC URL, block number
- [ ] Add specific error messages for common failure cases (tx not found, no orderbook events, etc.)
- [ ] Ensure CLI follows existing patterns for authentication and database setup
- [ ] Add `--dry-run` flag option to show what trade would be executed without placing order
- [ ] Update CLI tests to cover all validation scenarios and error cases
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`  
- [ ] Update TODOs.md with completion status

## Task 6. Update CLI Binary and Documentation

Update the CLI entry point and provide clear usage documentation:

- [ ] Update `@src/bin/cli.rs` to handle new command if needed
- [ ] Ensure `CliEnv::parse_and_convert` properly handles all required EVM environment variables
- [ ] Add usage examples to `CLAUDE.md` showing how to test opposite-side trades
- [ ] Update CLI help text to document all required environment variables (WS_RPC_URL, ORDERBOOK, etc.)
- [ ] Add troubleshooting section for common issues (invalid tx hash, network connection, auth failures)
- [ ] Test CLI with real transaction hashes on testnet if available
- [ ] Verify integration with existing database schema and migrations
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

## Implementation Notes

### Reusable Components Identified:
- **Symbol Cache**: `@src/symbol_cache.rs` - Thread-safe ERC20 symbol caching
- **Trade Conversion**: `@src/trade/mod.rs` - `PartialArbTrade` conversion logic from blockchain events
- **Order Execution**: `@src/schwab/order.rs` - Schwab API integration and order placement
- **Authentication**: `@src/schwab/tokens.rs` - OAuth token management
- **Database Integration**: `@src/arb.rs` - Trade persistence and status tracking

### Key Architecture Decisions:
1. **Shared Logic**: Extract transaction processing logic into `@src/trade/processor.rs` to be used by both main bot and CLI
2. **Provider Reuse**: Leverage existing alloy provider setup patterns from `@src/lib.rs`
3. **Error Consistency**: Use existing error types (`TradeConversionError`, `SchwabError`) for consistent error handling
4. **Database Integration**: Reuse existing `ArbTrade` struct and database schema for consistency
5. **CLI Patterns**: Follow existing CLI command patterns for authentication, validation, and output formatting

### Testing Strategy:
- **Unit Tests**: Mock all external dependencies (blockchain RPC, Schwab API) using existing test patterns
- **Integration Tests**: Test complete flow from transaction hash to trade execution using httpmock for Schwab API
- **Error Handling**: Comprehensive test coverage for all failure scenarios (invalid tx, network errors, auth failures)
- **Edge Cases**: Test edge cases like transactions with multiple orderbook events, invalid order configurations
