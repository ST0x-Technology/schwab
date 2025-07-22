# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust-based arbitrage bot for tokenized equities that monitors onchain trades via the Raindex orderbook and executes offsetting trades on Charles Schwab to maintain market-neutral positions. The bot bridges the gap between onchain tokenized equity markets and traditional brokerage platforms by exploiting price discrepancies.

## Key Development Commands

### Building & Running
- `cargo build` - Build the project
- `cargo run --bin main` - Run the main arbitrage bot
- `cargo run --bin auth` - Run the authentication flow for Charles Schwab OAuth setup

### Testing
- `cargo test` - Run all tests
- `cargo test --lib` - Run library tests only
- `cargo test --bin <binary>` - Run tests for specific binary
- `cargo test <test_name>` - Run specific test

### Database Management
- `sqlx migrate run` - Apply database migrations
- `sqlx migrate revert` - Revert last migration
- Database URL configured via `DATABASE_URL` environment variable

### Development Tools
- `rainix-rs-static` - Run Rust static analysis
- `cargo clippy` or `cargo-clippy` in Direnv - Run Clippy for linting
- `cargo fmt` - Format code
- `cargo-tarpaulin --skip-clean --out Html` - Generate test coverage report

### Nix Development Environment
- `nix develop` - Enter development shell with all dependencies
- `nix run .#prepSolArtifacts` - Build Solidity artifacts for orderbook interface
- `nix run .#checkTestCoverage` - Generate test coverage report

## Architecture Overview

### Core Event Processing Flow

**Main Event Loop ([`run` function in `src/lib.rs`])**
- Monitors two concurrent WebSocket event streams: `ClearV2` and `TakeOrderV2` from the Raindex orderbook
- Uses `tokio::select!` to handle events from either stream without blocking
- Converts blockchain events to structured `Trade` objects for processing

**Trade Conversion Logic ([`Trade` struct and methods in `src/trade/mod.rs`])**
- Parses onchain events into actionable trade data with strict validation
- Expects symbol pairs of USDC + tokenized equity with "s1" suffix (e.g., "AAPLs1")
- Determines Schwab trade direction: buying tokenized equity onchain → selling on Schwab
- Calculates prices in cents and maintains onchain/offchain trade ratios

**Async Event Processing Architecture**
- Each blockchain event spawns independent async execution flow
- Handles throughput mismatch: fast onchain events vs slower Schwab API calls
- No artificial concurrency limits - processes events as they arrive
- Flow: Parse Event → SQLite Deduplication Check → Schwab API Call → Record Result

### Authentication & API Integration

**Charles Schwab OAuth (`src/schwab.rs`)**
- OAuth 2.0 flow with 30-minute access tokens and 7-day refresh tokens
- Token storage and retrieval from SQLite database
- Comprehensive error handling for authentication failures

**Symbol Caching (`src/symbol_cache.rs`)**
- Thread-safe caching of ERC20 token symbols using `tokio::sync::RwLock`
- Prevents repeated RPC calls for the same token addresses

### Database Schema & Idempotency

**SQLite Tables:**
- `trades`: Stores trade attempts with onchain/offchain details and unique `(tx_hash, log_index)` constraint
- `schwab_auth`: Stores OAuth tokens with timestamps

**Idempotency Controls:**
- Uses `(tx_hash, log_index)` as unique identifier to prevent duplicate trade execution
- Trade status tracking: pending → completed/failed
- Retry logic with exponential backoff for failed trades

### Configuration

Environment variables (can be set via `.env` file):
- `DATABASE_URL`: SQLite database path
- `WS_RPC_URL`: WebSocket RPC endpoint for blockchain monitoring  
- `ORDERBOOK`: Raindex orderbook contract address
- `ORDER_HASH`: Target order hash to monitor for trades
- `APP_KEY`, `APP_SECRET`: Charles Schwab API credentials
- `REDIRECT_URI`: OAuth redirect URI (default: https://127.0.0.1)
- `BASE_URL`: Schwab API base URL (default: https://api.schwabapi.com)

### Charles Schwab Setup Process

1. Create Charles Schwab brokerage account (Charles Schwab International if outside US)
2. Register developer account at https://developer.schwab.com/
3. Set up as Individual Developer and request Trader API access
4. Include your Charles Schwab account number in the API access request
5. Wait 3-5 days for account linking approval

### Code Quality & Best Practices

- **Event-Driven Architecture**: Each trade spawns independent async task for maximum throughput
- **SQLite Persistence**: Embedded database for trade tracking and authentication tokens
- **Symbol Suffix Convention**: Tokenized equities use "s1" suffix to distinguish from base assets
- **Price Direction Logic**: Onchain buy = offchain sell (and vice versa) to maintain market-neutral positions
- **Comprehensive Error Handling**: Custom error types (`TradeConversionError`, `SchwabAuthError`) with proper propagation
- **Idiomatic Functional Programming**: Prefer iterator-based functional programming patterns over imperative loops unless it increases complexity
- **Comments**: Never leave redundant comments. Only use comments to explain complex logic. Generally, code should be self-documenting through clear naming, structure, and type modeling. If a comment is needed to explain what the code does, consider refactoring the code to make it clearer

### Testing Strategy

- **Mock Blockchain Interactions**: Uses `alloy::providers::mock::Asserter` for deterministic testing
- **HTTP API Mocking**: `httpmock` crate for Charles Schwab API testing
- **Database Isolation**: In-memory SQLite databases for test isolation
- **Edge Case Coverage**: Comprehensive error scenario testing for trade conversion logic
- **Testing Principle**: Only cover happy paths with all components working and connected in integration tests and cover everything in unit tests

### Code style

#### Use `.unwrap` over boolean result assertions in tests

Instead of

```rust
assert!(result.is_err());
assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
```

or

```rust
assert!(result.is_ok());
assert_eq!(result.unwrap(), "refreshed_access_token");
```

Write

```rust
assert!(matches!(result.unwrap_err(), SchwabAuthError::Reqwest(_)));
```

and

```rust
assert_eq!(result.unwrap(), "refreshed_access_token");
```

so that if we get an unexpected result value, we immediately see the value.

#### Avoid deep nesting

Prefer flat code over deeply nested blocks to improve readability and maintainability.

**Use early returns:**

```rust
fn process_data(data: Option<&str>) -> Result<String, Error> {
    let data = data.ok_or(Error::None)?;
    
    if data.is_empty() {
        return Err(Error::Empty);
    }
    
    if data.len() <= 5 {
        return Err(Error::TooShort);
    }
    
    Ok(data.to_uppercase())
}
```

**Extract functions for complex logic:**

```rust
fn validate_trade_data(trade: &Trade) -> Result<(), ValidationError> {
    validate_symbol(&trade.symbol)?;
    validate_quantity(trade.quantity)?;
    validate_price(trade.price)?;
    Ok(())
}
```

**Use pattern matching with guards:**

```rust
match (input, state) {
    (Some(data), State::Ready) if data.is_valid() => process(data),
    (Some(_), State::Ready) => Err(Error::InvalidData),
    (None, _) => Err(Error::NoData),
    _ => Err(Error::NotReady),
}
```

**Prefer iterator chains over nested loops:**

```rust
trades
    .iter()
    .filter(|t| t.is_valid())
    .map(|t| process_trade(t))
    .collect::<Result<Vec<_>, _>>()
```

#### Struct field access

Avoid creating unnecessary constructors or getters when they don't add logic beyond setting/getting field values. Use public fields directly instead.

**Prefer direct field access:**

```rust
pub struct SchwabTokens {
    pub access_token: String,
    pub access_token_fetched_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_token_fetched_at: DateTime<Utc>,
}

// Create with struct literal syntax
let tokens = SchwabTokens {
    access_token: "token123".to_string(),
    access_token_fetched_at: Utc::now(),
    refresh_token: "refresh456".to_string(),
    refresh_token_fetched_at: Utc::now(),
};

// Access fields directly
println!("Token: {}", tokens.access_token);
```

**Avoid unnecessary constructors and getters:**

```rust
// Don't create these unless they add meaningful logic
impl SchwabTokens {
    // Unnecessary - just sets fields without additional logic
    pub fn new(access_token: String, /* ... */) -> Self { /* ... */ }
    
    // Unnecessary - just returns field value
    pub fn access_token(&self) -> &str { &self.access_token }
}
```

This preserves argument clarity and avoids losing information about what each field represents.
