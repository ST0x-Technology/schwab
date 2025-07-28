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
- **Comments**: Follow comprehensive commenting guidelines (see detailed section below)
- **Spacing**: Leave an empty line in between code blocks to allow vim curly braces jumping between blocks and for easier reading

### Testing Strategy

- **Mock Blockchain Interactions**: Uses `alloy::providers::mock::Asserter` for deterministic testing
- **HTTP API Mocking**: `httpmock` crate for Charles Schwab API testing
- **Database Isolation**: In-memory SQLite databases for test isolation
- **Edge Case Coverage**: Comprehensive error scenario testing for trade conversion logic
- **Testing Principle**: Only cover happy paths with all components working and connected in integration tests and cover everything in unit tests

### Commenting Guidelines

Code should be primarily self-documenting through clear naming, structure, and type modeling. Comments should only be used when they add meaningful context that cannot be expressed through code structure alone.

#### When to Use Comments

**✅ DO comment when:**

- **Complex business logic**: Explaining non-obvious domain-specific rules or calculations
- **Algorithm rationale**: Why a particular approach was chosen over alternatives
- **External system interactions**: Behavior that depends on external APIs or protocols
- **Non-obvious technical constraints**: Performance considerations, platform limitations
- **Test data context**: Explaining what mock values represent or test scenarios
- **Workarounds**: Temporary solutions with context about why they exist

**❌ DON'T comment when:**

- The code is self-explanatory through naming and structure
- Restating what the code obviously does
- Describing function signatures (use doc comments instead)
- Adding obvious test setup descriptions
- Marking code sections that are clear from structure

#### Good Comment Examples

```rust
// If the on-chain order has USDC as input and an s1 tokenized stock as
// output then it means the order received USDC and gave away an s1  
// tokenized stock, i.e. sold, which means that to take the opposite
// trade in schwab we need to buy and vice versa.
let (schwab_ticker, schwab_instruction) = 
    if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("s1") {
        // ... complex mapping logic
    }

// We need to get the corresponding AfterClear event as ClearV2 doesn't
// contain the amounts. So we query the same block number, filter out
// logs with index lower than the ClearV2 log index and with tx hashes
// that don't match the ClearV2 tx hash.
let after_clear_logs = provider.get_logs(/* ... */).await?;

// Test data representing 9 shares with 18 decimal places
aliceOutput: U256::from_str("9000000000000000000").unwrap(), // 9 shares (18 dps)

/// Helper that converts a fixed-decimal `U256` amount into an `f64` using
/// the provided number of decimals.
///
/// NOTE: Parsing should never fail but precision may be lost.
fn u256_to_f64(amount: U256, decimals: u8) -> Result<f64, ParseFloatError> {
```

#### Bad Comment Examples

```rust
// ❌ Redundant - the function name says this
// Spawn background token refresh task
spawn_automatic_token_refresh(pool, env);

// ❌ Obvious from context
// Store test tokens
let tokens = SchwabTokens { /* ... */ };
tokens.store(&pool).await.unwrap();

// ❌ Just restating the code
// Mock account hash endpoint
let mock = server.mock(|when, then| {
    when.method(GET).path("/trader/v1/accounts/accountNumbers");
    // ...
});

// ❌ Test section markers that add no value
// 1. Test token refresh integration
let result = refresh_tokens(&pool).await;

// ❌ Explaining what the code obviously does
// Execute the order
execute_schwab_order(env, pool, trade).await;

// ❌ Obvious variable assignments
// Create a trade
let trade = Trade { /* ... */ };

// ❌ Test setup that's clear from code structure
// Verify mocks were called
mock.assert();

// ❌ Obvious control flow
// Save trade to DB
trade.save_to_db(&pool).await?;
```

#### Function Documentation

Use Rust doc comments (`///`) for public APIs:

```rust
/// Validates Schwab authentication tokens and refreshes if needed.
/// 
/// Returns `SchwabError::RefreshTokenExpired` if the refresh token
/// has expired and manual re-authentication is required.
pub async fn refresh_if_needed(pool: &SqlitePool) -> Result<bool, SchwabError> {
```

#### Comment Maintenance

- Remove comments when refactoring makes them obsolete
- Update comments when changing the logic they describe  
- If a comment is needed to explain what code does, consider refactoring for clarity
- Keep comments concise and focused on the "why" rather than the "what"

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

Instead of

```rust
fn process_data(data: Option<&str>) -> Result<String, Error> {
    if let Some(data) = data {
        if !data.is_empty() {
            if data.len() > 5 {
                Ok(data.to_uppercase())
            } else {
                Err(Error::TooShort)
            }
        } else {
            Err(Error::Empty)
        }
    } else {
        Err(Error::None)
    }
}
```

Write

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

Instead of

```rust
fn process_blockchain_event(event: &Event, db: &Database) -> Result<(), ProcessingError> {
    match event.event_type {
        EventType::ClearV2 => {
            if let Some(trade_data) = &event.trade_data {
                for trade in &trade_data.trades {
                    if trade.token_pair.len() == 2 {
                        if let (Some(token_a), Some(token_b)) = (&trade.token_pair[0], &trade.token_pair[1]) {
                            if token_a.symbol.ends_with("s1") || token_b.symbol.ends_with("s1") {
                                let (equity_token, usdc_token) = if token_a.symbol.ends_with("s1") {
                                    (token_a, token_b)
                                } else {
                                    (token_b, token_a)
                                };
                                
                                if usdc_token.symbol == "USDC" {
                                    if let Ok(existing) = db.find_trade(&event.tx_hash, event.log_index) {
                                        if existing.status != TradeStatus::Completed {
                                            // Process retry logic
                                            if existing.retry_count < 3 {
                                                match schwab_client.execute_trade(&trade) {
                                                    Ok(result) => {
                                                        db.update_trade_status(&existing.id, TradeStatus::Completed)?;
                                                    }
                                                    Err(e) => {
                                                        db.increment_retry_count(&existing.id)?;
                                                        if existing.retry_count >= 2 {
                                                            db.update_trade_status(&existing.id, TradeStatus::Failed)?;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        // New trade processing
                                        let new_trade = Trade::new(equity_token, usdc_token, &trade)?;
                                        db.insert_trade(&new_trade)?;
                                        match schwab_client.execute_trade(&new_trade) {
                                            Ok(result) => {
                                                db.update_trade_status(&new_trade.id, TradeStatus::Completed)?;
                                            }
                                            Err(e) => {
                                                db.update_trade_status(&new_trade.id, TradeStatus::Failed)?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        EventType::TakeOrderV2 => {
            // Similar deeply nested logic for TakeOrderV2...
        }
        _ => return Err(ProcessingError::UnsupportedEventType),
    }
    Ok(())
}
```

Write

```rust
fn process_blockchain_event(event: &Event, db: &Database) -> Result<(), ProcessingError> {
    match event.event_type {
        EventType::ClearV2 => process_clear_event(event, db),
        EventType::TakeOrderV2 => process_take_order_event(event, db),
        _ => Err(ProcessingError::UnsupportedEventType),
    }
}

fn process_clear_event(event: &Event, db: &Database) -> Result<(), ProcessingError> {
    let trade_data = event.trade_data.as_ref().ok_or(ProcessingError::NoTradeData)?;
    
    for trade in &trade_data.trades {
        if let Some((equity_token, usdc_token)) = extract_valid_token_pair(trade)? {
            handle_trade_processing(event, trade, equity_token, usdc_token, db)?;
        }
    }
    Ok(())
}

fn extract_valid_token_pair(trade: &TradeInfo) -> Result<Option<(&Token, &Token)>, ProcessingError> {
    if trade.token_pair.len() != 2 {
        return Ok(None);
    }
    
    let (token_a, token_b) = (&trade.token_pair[0], &trade.token_pair[1]);
    let (equity_token, usdc_token) = if token_a.symbol.ends_with("s1") {
        (token_a, token_b)
    } else if token_b.symbol.ends_with("s1") {
        (token_b, token_a)
    } else {
        return Ok(None);
    };
    
    if usdc_token.symbol == "USDC" {
        Ok(Some((equity_token, usdc_token)))
    } else {
        Ok(None)
    }
}

fn handle_trade_processing(
    event: &Event,
    trade: &TradeInfo, 
    equity_token: &Token,
    usdc_token: &Token,
    db: &Database
) -> Result<(), ProcessingError> {
    if let Ok(existing) = db.find_trade(&event.tx_hash, event.log_index) {
        handle_existing_trade(existing, trade, db)
    } else {
        handle_new_trade(event, trade, equity_token, usdc_token, db)
    }
}
```

**Use pattern matching with guards:**

Instead of

```rust
if let Some(data) = input {
    if state == State::Ready {
        if data.is_valid() {
            process(data)
        } else {
            Err(Error::InvalidData)
        }
    } else {
        Err(Error::NotReady)
    }
} else {
    if state == State::Ready {
        Err(Error::NoData)
    } else {
        Err(Error::NotReady)
    }
}
```

Write

```rust
match (input, state) {
    (Some(data), State::Ready) if data.is_valid() => process(data),
    (Some(_), State::Ready) => Err(Error::InvalidData),
    (None, _) => Err(Error::NoData),
    _ => Err(Error::NotReady),
}
```

**Prefer iterator chains over nested loops:**

Instead of

```rust
let mut results = Vec::new();
for trade in &trades {
    if trade.is_valid() {
        match process_trade(trade) {
            Ok(result) => results.push(result),
            Err(e) => return Err(e),
        }
    }
}
Ok(results)
```

Write

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