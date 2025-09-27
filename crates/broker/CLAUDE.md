# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Broker Crate Overview

This is the `st0x-broker` crate, a standalone library that provides a unified broker trait abstraction for executing stock trades across different brokers. Currently supports Charles Schwab via their API and includes a test broker for development/dry-run scenarios.

## Key Development Commands

### Building & Testing

- `cargo build` - Build the broker crate
- `cargo test -q` - Run all tests with minimal output
- `cargo test -q --lib` - Run library tests only  
- `cargo test -q <test_name>` - Run specific test
- `cargo test -q schwab` - Run tests containing "schwab"
- `cargo clippy --all-targets --all-features -- -D clippy::all` - Run linting (deny all warnings)
- `cargo fmt` - Format code

### Testing in Parent Workspace

Since this is a workspace member, you can also run from the parent directory:
- `cargo test -p st0x-broker` - Run tests for this crate only
- `cargo build -p st0x-broker` - Build this crate only

## Architecture Overview

### Core Broker Trait

The central `Broker` trait (`src/lib.rs`) defines the contract all broker implementations must follow:

- **Associated Types**: `Error`, `OrderId`, `Config` - allows each broker to define their own error types and configuration
- **Key Methods**: 
  - `try_from_config()` - Create and validate broker from config
  - `wait_until_market_open()` - Market hours checking
  - `place_market_order()` - Execute trades
  - `get_order_status()` - Check order status
  - `poll_pending_orders()` - Batch status updates

### Domain Types

**Type Safety via Newtypes** (`src/lib.rs`):
- `Symbol(String)` - Stock symbols with validation
- `Shares(u32)` - Share quantities with bounds checking  
- `Direction` - Buy/Sell enum with string conversion

**Order Modeling** (`src/order/mod.rs`):
- `MarketOrder` - Input for order placement
- `OrderPlacement<OrderId>` - Result of successful placement
- `OrderUpdate<OrderId>` - Status change notifications
- `OrderState` - Current order status (Submitted/Filled/Failed)

### Broker Implementations

**SchwabBroker** (`src/schwab/broker.rs`):
- OAuth 2.0 authentication with token refresh
- Market hours validation via Schwab API
- Order placement and status polling
- Database-backed token storage and order tracking

**TestBroker** (`src/test.rs`):
- Unified test/dry-run implementation
- Configurable failure scenarios
- Mock order execution with logging
- Always reports market as open

### Error Handling

**Hierarchical Error Design** (`src/lib.rs`, `src/error.rs`):
- `BrokerError` - Top-level broker errors with conversion from implementation-specific errors
- `PersistenceError` - Database and data corruption errors
- `SchwabError` - Schwab-specific API and authentication errors

## Code Quality Standards

### Type Modeling Best Practices

- **Algebraic Data Types**: Use enums to encode mutually exclusive states rather than multiple boolean fields
- **Newtypes**: Wrap primitives in domain-specific types to prevent mixing incompatible values
- **Typestate Pattern**: Encode object lifecycle in the type system where beneficial

### Testing Patterns

- **HTTP Mocking**: Uses `httpmock` for Schwab API testing
- **Database Isolation**: In-memory SQLite databases (`":memory:"`) for test isolation  
- **Test Utilities**: `test_utils.rs` provides shared setup functions
- **Meaningful Tests**: Tests verify business logic, not just language features

### Linting Policy

**CRITICAL**: Never add `#[allow(clippy::*)]` attributes or disable lints without explicit permission. When clippy reports issues:

1. **Refactor the code** to address the root cause
2. **Break down large functions** into smaller, focused functions
3. **Improve code structure** to meet clippy's standards
4. **Fix underlying issues** rather than suppressing warnings

### Database Integration

This crate expects SQLite database setup from the parent workspace:
- Migration files in `../../migrations/`
- Test setup via `sqlx::migrate!("../../migrations")`
- Schema focused on trade tracking and authentication

## Development Workflow

1. **Run tests first** - `cargo test -q` catches regressions early
2. **Address clippy warnings** - `cargo clippy --all-targets --all-features -- -D clippy::all`
3. **Format code last** - `cargo fmt` ensures consistent style
4. **Test behavior, not implementation** - Focus tests on business logic outcomes

## Architecture Integration

This crate is designed to be consumed by the parent arbitrage bot but maintains complete independence:
- No dependencies on parent crate types
- Self-contained error handling
- Standalone test suite
- Clear trait boundaries for multiple broker support

The `Broker` trait abstracts away broker-specific details, allowing the parent application to work with any implementation through the same interface.