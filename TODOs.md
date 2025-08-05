# CLI Implementation Tasks

## Task 1. Create CLI module with argument parsing
- [x] Create `src/cli.rs` with `clap` derive macros
- [x] Implement `buy` and `sell` subcommands
- [x] Add `-t/--ticker <SYMBOL>` and `-q/--quantity <AMOUNT>` flags
- [x] Handle potential flag conflicts by prioritizing CLI short flags
- [x] Add input validation for ticker symbols (uppercase, basic format checking)
- [x] Add quantity validation (positive numbers, fractional shares supported)
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 2. Create CLI binary entry point
- [x] Create `src/bin/cli.rs` with main function
- [x] Parse CLI arguments using the cli module
- [x] Load environment variables for Schwab authentication
- [x] Set up database connection and run migrations
- [x] Add comprehensive logging throughout all operations
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 3. Implement token refresh and order execution
- [x] Add token refresh at startup using `SchwabTokens::get_valid_access_token()`
- [x] Reuse existing `Order::new()` and `Order::place()` from `src/schwab/order.rs:23,44`
- [x] Add success/failure reporting with comprehensive logging
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 4. Implement OAuth flow for expired refresh tokens
- [x] Detect when refresh token has expired during CLI execution
- [x] Launch interactive OAuth flow using existing auth binary functionality
- [x] Guide user through authentication process with clear instructions
- [x] Retry the original operation after successful authentication
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 5. Implement comprehensive error handling and user feedback
- [x] Add contextual error messages for common failure scenarios
- [x] Handle network failures with retry suggestions
- [x] Handle invalid ticker symbols with helpful formatting hints
- [x] Handle insufficient account permissions with clear explanations
- [x] Add progress indicators for long-running operations
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 6. Write unit tests for CLI argument parsing
- [x] Test CLI argument parsing validation (invalid tickers, negative quantities, missing args)
- [x] Test input sanitization and validation logic  
- [x] Test error message formatting
- [x] Add direct tests for `Cli::parse_and_validate()` method
- [x] Add edge case tests for boundary conditions
- [x] Run `cargo test`
- [x] Run `cargo clippy -- -D clippy::all`
- [x] Run `cargo fmt`
- [x] Update @TODOs.md

## Task 7. Write integration tests for CLI commands
- [ ] Mock Schwab API responses for successful orders
- [ ] Mock authentication failures and token refresh scenarios
- [ ] Test database integration with in-memory SQLite
- [ ] Test end-to-end command execution with all components
- [ ] Test token refresh flow
- [ ] Run `cargo test`
- [ ] Run `cargo clippy -- -D clippy::all`
- [ ] Run `cargo fmt`
- [ ] Update @TODOs.md