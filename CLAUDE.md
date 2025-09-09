# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

## Plan & Review

### Before starting work

- Write a comprehensive step-by-step plan to
  .claude/tasks/YYYY-MM-DD-TASK_NAME.md with each task having a corresponding
  section and a list of subtasks as checkboxes inside of it
- The plan should be a detailed implementation plan and the reasoning behind the
  design decisions
- Do not include timelines in the plan as they tend to be inaccurate
- Remain focused on the task at hand, do not include unrelated improvements or
  premature optimizations
- Once you write the plan, ask me to review it. Do not continue until I approve
  the plan.

### While implementing

- You should update .claude/tasks/YYYY-MM-DD-TASK_NAME.md every time you
  complete a section
- Upon completing a planned task, add detailed descriptions of the changes you
  made to ease the review process

## Project Overview

This is a Rust-based arbitrage bot for tokenized equities that monitors onchain
trades via the Raindex orderbook and executes offsetting trades on Charles
Schwab to maintain market-neutral positions. The bot bridges the gap between
onchain tokenized equity markets and traditional brokerage platforms by
exploiting price discrepancies.

## Key Development Commands

### Building & Running

- `cargo build` - Build the project
- `cargo run --bin main` - Run the main arbitrage bot
- `cargo run --bin auth` - Run the authentication flow for Charles Schwab OAuth
  setup
- `cargo run --bin cli` - Run the command-line interface for manual operations

### Testing

- `cargo test -q` - Run all tests
- `cargo test -q --lib` - Run library tests only
- `cargo test -q --bin <binary>` - Run tests for specific binary
- `cargo test -q <test_name>` - Run specific test

### Database Management

- `sqlx db create` - Create the database
- `sqlx db drop` - Drop the database
- `sqlx db reset` - Drop database, recreate, and run pending migrations
- `sqlx db setup` - Create database and run pending migrations
- `sqlx migrate run` - Apply database migrations
- `sqlx migrate revert` - Revert last migration
- Database URL configured via `DATABASE_URL` environment variable

### Development Tools

- `rainix-rs-static` - Run Rust static analysis
- `cargo clippy --all-targets --all-features -- -D clippy::all` - Run Clippy for
  linting
- `cargo fmt` - Format code
- `cargo-tarpaulin --skip-clean --out Html` - Generate test coverage report

### Nix Development Environment

- `nix develop` - Enter development shell with all dependencies
- `nix run .#prepSolArtifacts` - Build Solidity artifacts for orderbook
  interface
- `nix run .#checkTestCoverage` - Generate test coverage report

## Development Workflow Notes

- When running `git diff`, make sure to add `--no-pager` to avoid opening it in
  the interactive view, e.g. `git --no-pager diff`

## Architecture Overview

### Core Event Processing Flow

**Main Event Loop (\[`run` function in `src/lib.rs`\])**

- Monitors two concurrent WebSocket event streams: `ClearV2` and `TakeOrderV2`
  from the Raindex orderbook
- Uses `tokio::select!` to handle events from either stream without blocking
- Converts blockchain events to structured `Trade` objects for processing

**Trade Conversion Logic (\[`Trade` struct and methods in
`src/trade/mod.rs`\])**

- Parses onchain events into actionable trade data with strict validation
- Expects symbol pairs of USDC + tokenized equity with "0x" suffix (e.g.,
  "AAPL0x")
- Determines Schwab trade direction: buying tokenized equity onchain → selling
  on Schwab
- Calculates prices in cents and maintains onchain/offchain trade ratios

**Async Event Processing Architecture**

- Each blockchain event spawns independent async execution flow
- Handles throughput mismatch: fast onchain events vs slower Schwab API calls
- No artificial concurrency limits - processes events as they arrive
- Flow: Parse Event → SQLite Deduplication Check → Schwab API Call → Record
  Result

### Authentication & API Integration

**Charles Schwab OAuth (`src/schwab.rs`)**

- OAuth 2.0 flow with 30-minute access tokens and 7-day refresh tokens
- Token storage and retrieval from SQLite database
- Comprehensive error handling for authentication failures

**Symbol Caching (`crate::symbol::cache::SymbolCache`)**

- Thread-safe caching of ERC20 token symbols using `tokio::sync::RwLock`
- Prevents repeated RPC calls for the same token addresses

### Database Schema & Idempotency

**SQLite Tables:**

- `onchain_trades`: Immutable blockchain trade records

  - `id`: Primary key (auto-increment)
  - `tx_hash`: Transaction hash (66 chars, 0x-prefixed)
  - `log_index`: Event log index (non-negative)
  - `symbol`: Asset symbol (non-empty string)
  - `amount`: Trade quantity (positive real number)
  - `direction`: Trade direction ('BUY' or 'SELL')
  - `price_usdc`: Price in USDC (positive real number)
  - `created_at`: Timestamp (default CURRENT_TIMESTAMP)
  - Unique constraint: `(tx_hash, log_index)`

- `schwab_executions`: Schwab order execution tracking

  - `id`: Primary key (auto-increment)
  - `symbol`: Asset symbol (non-empty string)
  - `shares`: Whole shares executed (positive integer)
  - `direction`: Execution direction ('BUY' or 'SELL')
  - `order_id`: Schwab order ID (nullable, non-empty if present)
  - `price_cents`: Execution price in cents (nullable, non-negative)
  - `status`: Execution status ('PENDING', 'COMPLETED', 'FAILED')
  - `executed_at`: Execution timestamp (nullable)
  - Check constraints ensure consistent status transitions

- `trade_accumulators`: Unified position tracking per symbol

  - `symbol`: Primary key (non-empty string)
  - `net_position`: Running net position (real number)
  - `accumulated_long`: Fractional shares for buying (non-negative)
  - `accumulated_short`: Fractional shares for selling (non-negative)
  - `pending_execution_id`: Reference to pending execution (nullable)
  - `last_updated`: Last update timestamp (default CURRENT_TIMESTAMP)

- `trade_execution_links`: Many-to-many audit trail

  - `id`: Primary key (auto-increment)
  - `trade_id`: Foreign key to onchain_trades
  - `execution_id`: Foreign key to schwab_executions
  - `contributed_shares`: Fractional shares contributed (positive)
  - `created_at`: Link creation timestamp
  - Unique constraint: `(trade_id, execution_id)`

- `schwab_auth`: OAuth token storage (sensitive data)

  - `id`: Primary key (constrained to 1 for singleton)
  - `access_token`: Current access token
  - `access_token_fetched_at`: Access token timestamp
  - `refresh_token`: Current refresh token
  - `refresh_token_fetched_at`: Refresh token timestamp

- `event_queue`: Idempotent event processing queue

  - `id`: Primary key (auto-increment)
  - `tx_hash`: Transaction hash (66 chars, 0x-prefixed)
  - `log_index`: Event log index (non-negative)
  - `block_number`: Block number (non-negative)
  - `event_data`: JSON serialized event (non-empty)
  - `processed`: Processing status (boolean, default false)
  - `created_at`: Queue entry timestamp
  - `processed_at`: Processing completion timestamp (nullable)
  - Unique constraint: `(tx_hash, log_index)`

- `symbol_locks`: Per-symbol execution concurrency control

  - `symbol`: Primary key (non-empty string)
  - `locked_at`: Lock acquisition timestamp

**Idempotency Controls:**

- Uses `(tx_hash, log_index)` as unique identifier to prevent duplicate trade
  execution
- Trade status tracking: pending → completed/failed
- Retry logic with exponential backoff for failed trades

### Configuration

Environment variables (can be set via `.env` file):

- `DATABASE_URL`: SQLite database path
- `WS_RPC_URL`: WebSocket RPC endpoint for blockchain monitoring
- `ORDERBOOK`: Raindex orderbook contract address
- `ORDER_OWNER`: Owner address of orders to monitor for trades
- `APP_KEY`, `APP_SECRET`: Charles Schwab API credentials
- `REDIRECT_URI`: OAuth redirect URI (default: https://127.0.0.1)
- `BASE_URL`: Schwab API base URL (default: https://api.schwabapi.com)

### Charles Schwab Setup Process

1. Create Charles Schwab brokerage account (Charles Schwab International if
   outside US)
2. Register developer account at https://developer.schwab.com/
3. Set up as Individual Developer and request Trader API access
4. Include your Charles Schwab account number in the API access request
5. Wait 3-5 days for account linking approval

### Code Quality & Best Practices

- **Event-Driven Architecture**: Each trade spawns independent async task for
  maximum throughput
- **SQLite Persistence**: Embedded database for trade tracking and
  authentication tokens
- **Symbol Suffix Convention**: Tokenized equities use "0x" suffix to
  distinguish from base assets
- **Price Direction Logic**: Onchain buy = offchain sell (and vice versa) to
  maintain market-neutral positions
- **Comprehensive Error Handling**: Custom error types (`OnChainError`,
  `SchwabError`) with proper propagation
- **Type Modeling**: Make invalid states unrepresentable through the type
  system. Use algebraic data types (ADTs) and enums to encode business rules and
  state transitions directly in types rather than relying on runtime validation.
  Examples:
  - Use enum variants to represent mutually exclusive states instead of multiple
    boolean flags
  - Encode state-specific data within enum variants rather than using nullable
    fields
  - Use newtypes for domain concepts to prevent mixing incompatible values
  - Leverage the type system to enforce invariants at compile time
- **Schema Design**: Avoid database columns that can contradict each other. Use
  constraints and proper normalization to ensure data consistency at the
  database level. Align database schemas with type modeling principles where
  possible
- **Functional Programming Patterns**: Favor FP and ADT patterns over OOP
  patterns. Avoid unnecessary encapsulation, inheritance hierarchies, or
  getter/setter patterns that don't make sense with Rust's algebraic data types.
  Use pattern matching, combinators, and type-driven design
- **Idiomatic Functional Programming**: Prefer iterator-based functional
  programming patterns over imperative loops unless it increases complexity. Use
  itertools to be able to do more with iterators and functional programming in
  Rust
- **Comments**: Follow comprehensive commenting guidelines (see detailed section
  below)
- **Spacing**: Leave an empty line in between code blocks to allow vim curly
  braces jumping between blocks and for easier reading
- **Import Conventions**: Use qualified imports when they prevent ambiguity
  (e.g. `contract::Error` for `alloy::contract::Error`), but avoid them when the
  module is clear (e.g. use `info!` instead of `tracing::info!`). Generally
  avoid imports inside functions. We don't do function-level imports, instead we
  do top-of-module imports. Note that I said top-of-module and not top-of-file,
  e.g. imports required only inside a tests module should be done in the module
  and not hidden behind #[cfg(test)] at the top of the file
- **Error Handling**: Avoid `unwrap()` even post-validation since validation
  logic changes might leave panics in the codebase
- **Visibility Levels**: Always keep visibility levels as restrictive as
  possible (prefer `pub(crate)` over `pub`, private over `pub(crate)`) to enable
  better dead code detection by the compiler and tooling. This makes the
  codebase easier to navigate and understand by making the relevance scope
  explicit

### CRITICAL: Financial Data Integrity

**This is a mission-critical financial application. The following patterns are
STRICTLY FORBIDDEN and can result in catastrophic financial losses:**

#### ❌ NEVER: Silent Error Masking or Data Corruption

**NEVER** write code that silently provides wrong values, hides conversion
errors, or masks failures in any way. This includes but is not limited to:

- Defensive value capping that hides overflow/underflow
- Fallback to default values on conversion failure
- Silent truncation of precision
- Using `unwrap_or(default_value)` on financial calculations
- Using `unwrap_or_default()` on monetary values
- Conversion functions that "gracefully degrade" instead of failing

**Example of FORBIDDEN patterns:**

```rust
// ❌ CATASTROPHICALLY DANGEROUS - Silent data corruption
const fn shares_to_db_i64(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX  // WRONG: Silently caps at wrong value
    } else {
        value as i64
    }
}

// ❌ DANGEROUS - Hides conversion errors
fn parse_price(input: &str) -> f64 {
    input.parse().unwrap_or(0.0)  // WRONG: 0.0 is not a safe fallback for prices
}

// ❌ DANGEROUS - Silent precision loss
fn convert_to_cents(dollars: f64) -> i64 {
    (dollars * 100.0) as i64  // WRONG: Truncates fractional cents
}

// ❌ DANGEROUS - Masks database constraint violations
async fn save_trade_amount(amount: Decimal, pool: &Pool) -> Result<(), Error> {
    let safe_amount = amount.min(Decimal::MAX).max(Decimal::ZERO);  // WRONG
    sqlx::query!("INSERT INTO trades (amount) VALUES (?)", safe_amount)
        .execute(pool).await?;
    Ok(())
}
```

#### ✅ REQUIRED: Explicit Error Handling

**ALL financial operations must use explicit error handling with proper error
propagation:**

```rust
// ✅ CORRECT - Explicit conversion with proper error handling
fn shares_to_db_i64(value: u64) -> Result<i64, ConversionError> {
    value.try_into()
        .map_err(|_| ConversionError::ValueTooLarge { 
            value, 
            max_allowed: i64::MAX as u64 
        })
}

// ✅ CORRECT - Parse with explicit error
fn parse_price(input: &str) -> Result<Decimal, ParseError> {
    Decimal::from_str(input)
        .map_err(|e| ParseError::InvalidPrice { 
            input: input.to_string(), 
            source: e 
        })
}

// ✅ CORRECT - Checked arithmetic for precision-critical operations
fn convert_to_cents(dollars: Decimal) -> Result<i64, ArithmeticError> {
    let cents = dollars.checked_mul(Decimal::from(100))
        .ok_or(ArithmeticError::Overflow)?;
    
    if cents.fract() != Decimal::ZERO {
        return Err(ArithmeticError::FractionalCents { value: cents });
    }
    
    cents.to_i64()
        .ok_or(ArithmeticError::ConversionFailed { value: cents })
}

// ✅ CORRECT - Let database constraints fail naturally
async fn save_trade_amount(amount: Decimal, pool: &Pool) -> Result<(), DatabaseError> {
    sqlx::query!("INSERT INTO trades (amount) VALUES (?)", amount)
        .execute(pool)
        .await
        .map_err(DatabaseError::from)?;
    Ok(())
}
```

#### Error Categories That Must Fail Fast

1. **Numeric Conversions**: Any conversion between numeric types must use
   `try_into()` or equivalent
2. **Precision Loss**: Operations that could lose precision must be explicit
   about it
3. **Range Violations**: Values outside expected ranges must error, not clamp
4. **Parse Failures**: String-to-number parsing must propagate parse errors
5. **Arithmetic Operations**: Use checked arithmetic for all financial
   calculations
6. **Database Constraints**: Let database constraints fail rather than masking
   violations

#### Required Error Types

Every financial operation must have proper error types that preserve context:

```rust
#[derive(Debug, thiserror::Error)]
pub enum FinancialError {
    #[error("Value {value} exceeds maximum allowed {max_allowed}")]
    ValueTooLarge { value: u64, max_allowed: u64 },
    
    #[error("Arithmetic overflow in operation: {operation}")]
    ArithmeticOverflow { operation: String },
    
    #[error("Precision loss detected: {original} -> {converted}")]
    PrecisionLoss { original: String, converted: String },
    
    #[error("Invalid price format: '{input}'")]
    InvalidPrice { input: String, #[source] source: DecimalError },
}
```

**Remember: In financial applications, it is ALWAYS better for the system to
fail fast with a clear error than to continue with potentially corrupted data.
Silent data corruption in financial systems can lead to massive losses,
regulatory violations, and complete system failure.**

### Testing Strategy

- **Mock Blockchain Interactions**: Uses `alloy::providers::mock::Asserter` for
  deterministic testing
- **HTTP API Mocking**: `httpmock` crate for Charles Schwab API testing
- **Database Isolation**: In-memory SQLite databases for test isolation
- **Edge Case Coverage**: Comprehensive error scenario testing for trade
  conversion logic
- **Testing Principle**: Only cover happy paths with all components working and
  connected in integration tests and cover everything in unit tests
- **Debugging failing tests**: When debugging tests with failing assert! macros,
  add additional context to the assert! macro instead of adding temporary
  println! statements
- **Test Quality**: Never write tests that only exercise language features
  without testing our application logic. Tests should verify actual business
  logic, not just struct field assignments or basic language operations

#### Writing Meaningful Tests

Tests should verify our application logic, not just language features. Avoid
tests that only exercise struct construction or field access without testing any
business logic.

**❌ Bad: Testing language features instead of our code**

```rust
#[test]
fn test_order_poller_config_custom() {
    let config = OrderPollerConfig {
        polling_interval: Duration::from_secs(30),
        max_jitter: Duration::from_secs(10),
    };

    assert_eq!(config.polling_interval, Duration::from_secs(30));
    assert_eq!(config.max_jitter, Duration::from_secs(10));
}
```

This test creates a struct and verifies field assignments, but doesn't test any
of our code logic - it only tests Rust's struct field assignment mechanism.

**✅ Good: Testing actual business logic**

```rust
#[test]
fn test_order_poller_respects_jitter_bounds() {
    let config = OrderPollerConfig {
        polling_interval: Duration::from_secs(60),
        max_jitter: Duration::from_secs(10),
    };
    
    let actual_delay = config.calculate_next_poll_delay();
    
    assert!(actual_delay >= Duration::from_secs(60));
    assert!(actual_delay <= Duration::from_secs(70));
}
```

This test verifies that our jitter calculation logic works correctly within
expected bounds.

### Workflow Best Practices

- **Always run tests, clippy, and pre-commit before handing over a piece of
  work**
  - Run tests first, as changing tests can break clippy
  - Run clippy next, as fixing linting errors can break formatting
  - Deny warnings when running clippy
  - Always run `pre-commit run -a` last to ensure git hooks pass. If it failed
    due to a formatting change then running `pre-commit run -a` again should
    pass

#### CRITICAL: Lint Policy

**NEVER add `#[allow(clippy::*)]` attributes or disable any lints without
explicit permission.** This is strictly forbidden. When clippy reports issues,
you MUST fix the underlying code problems, not suppress the warnings.

**Required approach for clippy issues:**

1. **Refactor the code** to address the root cause of the lint violation
2. **Break down large functions** into smaller, more focused functions
3. **Improve code structure** to meet clippy's standards
4. **Use proper error handling** instead of suppressing warnings

**Examples of FORBIDDEN practices:**

```rust
// ❌ NEVER DO THIS - Suppressing lints is forbidden
#[allow(clippy::too_many_lines)]
fn large_function() { /* ... */ }

#[allow(clippy::needless_continue)]
// ❌ NEVER DO THIS - Fix the code structure instead
```

**Required approach:**

```rust
// ✅ CORRECT - Refactor to address the issue
fn process_data() -> Result<(), Error> {
    let data = get_data()?;
    validate_data(&data)?;
    save_data(&data)?;
    Ok(())
}

fn validate_data(data: &Data) -> Result<(), Error> {
    // Extracted validation logic
}

fn save_data(data: &Data) -> Result<(), Error> {
    // Extracted saving logic
}
```

**If you encounter a clippy issue:**

1. Understand WHY clippy is flagging the code
2. Refactor the code to address the underlying problem
3. If you believe a lint is incorrect, ask for permission before suppressing it
4. Document your reasoning if given permission to suppress a specific lint

This policy ensures code quality remains high and prevents technical debt
accumulation through lint suppression.

### Commenting Guidelines

Code should be primarily self-documenting through clear naming, structure, and
type modeling. Comments should only be used when they add meaningful context
that cannot be expressed through code structure alone.

#### When to Use Comments

**✅ DO comment when:**

- **Complex business logic**: Explaining non-obvious domain-specific rules or
  calculations
- **Algorithm rationale**: Why a particular approach was chosen over
  alternatives
- **External system interactions**: Behavior that depends on external APIs or
  protocols
- **Non-obvious technical constraints**: Performance considerations, platform
  limitations
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
// If the on-chain order has USDC as input and an 0x tokenized stock as
// output then it means the order received USDC and gave away an 0x  
// tokenized stock, i.e. sold, which means that to take the opposite
// trade in schwab we need to buy and vice versa.
let (schwab_ticker, schwab_instruction) = 
    if onchain_input_symbol == "USDC" && onchain_output_symbol.ends_with("0x") {
        // ... complex mapping logic
    }

// We need to get the corresponding AfterClear event as ClearV2 doesn't
// contain the amounts. So we query the same block number, filter out
// logs with index lower than the ClearV2 log index and with tx hashes
// that don't match the ClearV2 tx hash.
let after_clear_logs = provider.get_logs(/* ... */).await?;

// Test data representing 9 shares with 18 decimal places
alice_output: U256::from_str("9000000000000000000").unwrap(), // 9 shares (18 dps)

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
trade.try_save_to_db(&pool).await?;
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
- If a comment is needed to explain what code does, consider refactoring for
  clarity
- Keep comments concise and focused on the "why" rather than the "what"

### Code style

#### Use `.unwrap` over boolean result assertions in tests

Instead of

```rust
assert!(result.is_err());
assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
```

or

```rust
assert!(result.is_ok());
assert_eq!(result.unwrap(), "refreshed_access_token");
```

Write

```rust
assert!(matches!(result.unwrap_err(), SchwabError::Reqwest(_)));
```

and

```rust
assert_eq!(result.unwrap(), "refreshed_access_token");
```

so that if we get an unexpected result value, we immediately see the value.

#### Type modeling examples

**Make invalid states unrepresentable:**

Instead of using multiple fields that can contradict each other:

```rust
// ❌ Bad: Multiple fields can be in invalid combinations
pub struct Order {
    pub status: String,  // "pending", "completed", "failed"
    pub order_id: Option<String>,  // Some when completed, None when pending
    pub executed_at: Option<DateTime<Utc>>,  // Some when completed
    pub price_cents: Option<i64>,  // Some when completed
    pub error_reason: Option<String>,  // Some when failed
}
```

Use enum variants to encode valid states:

```rust
// ✅ Good: Each state has exactly the data it needs
pub enum OrderStatus {
    Pending,
    Completed {
        order_id: String,
        executed_at: DateTime<Utc>,
        price_cents: i64,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error_reason: String,
    },
}
```

**Use newtypes for domain concepts:**

```rust
// ❌ Bad: Easy to mix up parameters of the same type
fn place_order(symbol: String, account: String, amount: i64, price: i64) { }

// ✅ Good: Type system prevents mixing incompatible values
#[derive(Debug, Clone)]
struct Symbol(String);

#[derive(Debug, Clone)]
struct AccountId(String);

#[derive(Debug)]
struct Shares(i64);

#[derive(Debug)]
struct PriceCents(i64);

fn place_order(symbol: Symbol, account: AccountId, amount: Shares, price: PriceCents) { }
```

**The Typestate Pattern:**

The typestate pattern encodes information about an object's runtime state in its
compile-time type. This moves state-related errors from runtime to compile time,
eliminating runtime checks and making illegal states unrepresentable.

```rust
// ✅ Good: Typestate pattern with zero-cost state transitions
struct Start;
struct InProgress;
struct Complete;

// Generic struct with state parameter
struct Task<State> {
    data: TaskData,
    state: State,  // Can store state-specific data
}

// Operations only available in Start state
impl Task<Start> {
    fn new() -> Self {
        Task { data: TaskData::new(), state: Start }
    }
    
    fn begin(self) -> Task<InProgress> {
        // Consumes self, returns new state
        Task { data: self.data, state: InProgress }
    }
}

// Operations only available in InProgress state
impl Task<InProgress> {
    fn work(&mut self) {
        // Can mutate without changing state
    }
    
    fn complete(self) -> Task<Complete> {
        // State transition consumes self
        Task { data: self.data, state: Complete }
    }
}

// Operations available in multiple states
impl<S> Task<S> {
    fn description(&self) -> &str {
        &self.data.description
    }
}
```

**Session Types and Protocol Enforcement:**

```rust
// ✅ Good: Enforce protocol sequences at compile time
struct Unauthenticated;
struct Authenticated { token: String };
struct Active { token: String, session_id: u64 };

struct Connection<State> {
    socket: TcpStream,
    state: State,
}

impl Connection<Unauthenticated> {
    fn authenticate(self, credentials: &Credentials) 
        -> Result<Connection<Authenticated>, AuthError> {
        let token = perform_auth(&self.socket, credentials)?;
        Ok(Connection {
            socket: self.socket,
            state: Authenticated { token },
        })
    }
}

impl Connection<Authenticated> {
    fn start_session(self) -> Connection<Active> {
        let session_id = generate_session_id();
        Connection {
            socket: self.socket,
            state: Active { 
                token: self.state.token,
                session_id,
            },
        }
    }
}

impl Connection<Active> {
    fn send_message(&mut self, msg: &Message) {
        // Can only send messages in active state
    }
}
```

**Builder Pattern with Typestate:**

```rust
// ✅ Good: Can't build incomplete objects at compile time
struct NoUrl;
struct HasUrl;
struct NoMethod;
struct HasMethod;

struct RequestBuilder<U, M> {
    url: Option<String>,
    method: Option<Method>,
    headers: Vec<Header>,
    _url: PhantomData<U>,
    _method: PhantomData<M>,
}

impl RequestBuilder<NoUrl, NoMethod> {
    fn new() -> Self {
        RequestBuilder {
            url: None,
            method: None,
            headers: Vec::new(),
            _url: PhantomData,
            _method: PhantomData,
        }
    }
}

impl<M> RequestBuilder<NoUrl, M> {
    fn url(self, url: String) -> RequestBuilder<HasUrl, M> {
        RequestBuilder {
            url: Some(url),
            method: self.method,
            headers: self.headers,
            _url: PhantomData,
            _method: PhantomData,
        }
    }
}

impl<U> RequestBuilder<U, NoMethod> {
    fn method(self, method: Method) -> RequestBuilder<U, HasMethod> {
        RequestBuilder {
            url: self.url,
            method: Some(method),
            headers: self.headers,
            _url: PhantomData,
            _method: PhantomData,
        }
    }
}

// Can only build when we have both URL and method
impl RequestBuilder<HasUrl, HasMethod> {
    fn build(self) -> Request {
        Request {
            url: self.url.unwrap(), // Safe due to typestate
            method: self.method.unwrap(), // Safe due to typestate
            headers: self.headers,
        }
    }
}

// Usage: won't compile without setting both url and method
let request = RequestBuilder::new()
    .url("https://api.example.com".into())
    .method(Method::GET)
    .build();
```

#### Avoid deep nesting

Prefer flat code over deeply nested blocks to improve readability and
maintainability.

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

**Use let-else pattern for guard clauses:**

The let-else pattern (available since Rust 1.65) is excellent for reducing
nesting when you need to extract a value or return early:

Instead of

```rust
fn process_event(event: &QueuedEvent) -> Result<Trade, Error> {
    if let Some(trade_data) = convert_event_to_trade(event) {
        if trade_data.is_valid() {
            if let Some(symbol) = trade_data.extract_symbol() {
                Ok(Trade::new(symbol, trade_data))
            } else {
                Err(Error::NoSymbol)
            }
        } else {
            Err(Error::InvalidTrade)
        }
    } else {
        Err(Error::ConversionFailed)
    }
}
```

Write

```rust
fn process_event(event: &QueuedEvent) -> Result<Trade, Error> {
    let Some(trade_data) = convert_event_to_trade(event) else {
        return Err(Error::ConversionFailed);
    };
    
    if !trade_data.is_valid() {
        return Err(Error::InvalidTrade);
    }
    
    let Some(symbol) = trade_data.extract_symbol() else {
        return Err(Error::NoSymbol);
    };
    
    Ok(Trade::new(symbol, trade_data))
}
```

This pattern is particularly useful for:

- Extracting required values from Options
- Handling pattern matching that should cause early returns
- Reducing rightward drift in functions with multiple validation steps

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
                            if token_a.symbol.ends_with("0x") || token_b.symbol.ends_with("0x") {
                                let (equity_token, usdc_token) = if token_a.symbol.ends_with("0x") {
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
    let (equity_token, usdc_token) = if token_a.symbol.ends_with("0x") {
        (token_a, token_b)
    } else if token_b.symbol.ends_with("0x") {
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

Avoid creating unnecessary constructors or getters when they don't add logic
beyond setting/getting field values. Use public fields directly instead.

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

This preserves argument clarity and avoids losing information about what each
field represents.
