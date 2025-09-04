# 2025-09-03 Market Hours Control

This file documents the plan for implementing market hours tracking and control
for the arbitrage bot, ensuring it starts/stops the entire bot flow based on
Schwab market hours.

## Task 1: Create Market Hours API Client Module ✅ COMPLETED

### Problem Summary

The bot currently runs continuously 24/7. We need to implement market hours
tracking using the Schwab API so the bot only runs during market hours,
completely stopping during closed hours.

### Implementation Checklist

- [x] Create `src/schwab/market_hours.rs` module
- [x] Define data structures using ADTs and enums per CLAUDE.md guidelines:
  - [x] `MarketHours` struct with fields for date, session type, start/end
        times, is_open
  - [x] `MarketSession` enum (PreMarket, Regular, AfterHours) - no string types
  - [x] `MarketStatus` enum (Open, Closed) - make invalid states unrepresentable
- [x] Implement Schwab Market Data API client:
  - [x] `fetch_market_hours()` - calls `/marketdata/v1/markets/{marketId}`
        endpoint
  - [x] Handle authentication for Market Data API (reuse existing token
        management)
  - [x] Parse API responses with explicit error handling (no unwrap_or patterns)
  - [x] Return `Result<MarketHours, SchwabError>` - fail fast on API errors
- [x] Implement timezone handling using chrono-tz (ET to system timezone)
- [x] Add comprehensive test coverage with meaningful tests (not just field
      assignment)

### Implementation Details

**Module Structure:**

- Added `chrono-tz` dependency via `cargo add chrono-tz`
- Created `src/schwab/market_hours.rs` with proper visibility (`pub(crate)`)
- Updated `src/schwab/mod.rs` to export the new module

**Data Structures (ADT/Enum Design):**

```rust
// Strong typing prevents invalid states
pub(crate) enum MarketSession {
    PreMarket,
    Regular,
    AfterHours,
}

pub(crate) enum MarketStatus {
    Open,
    Closed,
}

// Complete market hours information with timezone support
pub(crate) struct MarketHours {
    pub date: NaiveDate,
    pub session_type: MarketSession,
    pub start: Option<DateTime<Tz>>,  // None for closed days
    pub end: Option<DateTime<Tz>>,    // None for closed days
    pub is_open: bool,
}
```

**API Integration:**

- Follows existing patterns from `auth.rs` and `order.rs`
- Uses `SchwabTokens::get_valid_access_token()` for authentication
- Implements retry logic with `backon::ExponentialBuilder`
- Calls `/marketdata/v1/markets/equity` endpoint
- Proper error propagation with `SchwabError::RequestFailed`
- No `unwrap_or` patterns - all errors fail fast explicitly

**Timezone Handling:**

- Uses `chrono_tz::US::Eastern` for market timezone
- Provides conversion methods: `start_in_local()`, `end_in_local()`
- Handles DST transitions correctly via chrono-tz
- `current_status()` method determines real-time market state

**Test Coverage:**

- 13 comprehensive tests covering:
  - API response parsing (open/closed market scenarios)
  - Error handling (API failures, invalid responses)
  - Timezone conversions and edge cases
  - Business logic validation (not just struct assignment)
  - Enum serialization/deserialization

**Code Quality:**

- All tests passing (350 total tests)
- Clippy linting passes with no errors
- Pre-commit hooks all passing
- Follows CLAUDE.md financial error handling rules (no unwrap patterns)
- Proper visibility restrictions (`pub(crate)` only)

**Bug Fix (2025-09-04):**

- ✅ **FIXED**: Corrected API endpoint from
  `/marketdata/v1/markets/equity/hours` to `/marketdata/v1/markets/equity`
- The original endpoint was returning 404 Not Found
- Research confirmed correct Schwab Market Data API endpoints:
  - Single market: `GET /marketdata/v1/markets/{marketId}`
  - Multiple markets: `GET /marketdata/v1/markets?markets={list}`
- **Verification**: CLI command now works successfully:
  ```
  $ cargo run --bin cli market-status
  Market Status: CLOSED
  Wednesday, September 03, 2025: Regular Hours: 09:30 AM ET - 04:00 PM ET
  ```

### API Integration Details

Based on research, the Schwab Market Data API provides:

- **Endpoint**: `GET /marketdata/v1/markets/{marketId}`
- **Parameters**:
  - `marketId`: "equity" for US equities
  - `date`: Optional YYYY-MM-DD format (defaults to current day)
- **Response**: Market hours data including session types and times
- **Note**: API returns no hours or is_open=false for holidays/weekends

### Success Criteria

- Can fetch current day's market hours from Schwab API
- Correctly handles timezones (ET to system timezone conversion)
- Properly identifies closed days from API response
- Explicit error propagation with no silent failures
- Test coverage verifies business logic, not language features

## Task 2: Add CLI Command for Market Status ✅ COMPLETED

### Problem Summary

Need a CLI command to check if the market is currently open, which will test the
market hours logic independently before integrating into the main bot flow.

### Implementation Checklist

- [x] Add new CLI command `market-status` to `src/cli.rs` using clap
- [x] Command implementation:
  - [x] Fetch current market hours using the same logic as main bot will use
  - [x] Display current market status (OPEN/CLOSED)
  - [x] Show today's market hours if available
  - [x] Display time until next state change
  - [x] Support optional `--date` parameter for checking specific dates
- [x] Reuse `schwab::market_hours` module from Task 1
- [x] Use proper error types - no string errors, use thiserror enums

### Example Output

```
$ cargo run --bin cli market-status
Market Status: CLOSED
Today: Saturday, September 4, 2025 (Market Closed)
Next Open: Monday, September 6, 2025 at 9:30 AM ET (in 1d 14h 23m)

$ cargo run --bin cli market-status --date 2025-09-06
Market Status for September 6, 2025: 
Regular Hours: 9:30 AM - 4:00 PM ET
```

### Implementation Details

**CLI Integration:**

- Added `MarketStatus` variant to `Commands` enum with optional `--date`
  parameter
- Integrated into existing CLI command structure in `src/cli.rs`
- Follows existing patterns for error handling and stdout formatting

**Display Logic:**

- Created `display_market_status` function that formats market hours information
- Shows current market status (OPEN/CLOSED)
- Displays market hours in Eastern timezone with formatted dates
- Calculates and shows time until market opens/closes
- Handles closed market days (weekends/holidays) gracefully

**Testing:**

- Added comprehensive integration tests covering:
  - Open market scenarios with time calculations
  - Closed market scenarios (weekends/holidays)
  - Authentication failures
  - API error handling
  - CLI help text integration
- All tests pass with proper mock server responses

**Code Quality:**

- Passes all clippy lints
- Uses typed errors with proper propagation
- Follows CLAUDE.md guidelines for error handling
- No unwrap patterns - explicit error handling throughout

### Success Criteria

- ✅ Command provides clear, accurate market status information
- ✅ Proper error handling with typed errors
- ✅ Shows meaningful information for weekends/holidays
- ✅ Can verify market hours logic before full integration
- ✅ Comprehensive test coverage
- ✅ CLI help integration verified

## Task 3: Create Market Hours Cache Module ✅ COMPLETED

### Problem Summary

To avoid excessive API calls, we need to cache market hours in memory similar to
the existing `SymbolCache`.

### Implementation Checklist

- [x] Create `src/schwab/market_hours_cache.rs` module following SymbolCache
      pattern
- [x] Implement `MarketHoursCache` struct:
  - [x] Use `tokio::sync::RwLock` for thread-safe access
  - [x] Store `HashMap<(String, NaiveDate), MarketHours>`
  - [x] Cache only today's and tomorrow's hours (minimal memory)
- [x] Implement methods following functional programming patterns:
  - [x] `get_or_fetch()` - check cache first, fetch if missing
  - [x] `get_current_status()` - return current market status
  - [x] `get_next_transition()` - return next open/close time
- [x] Cache refresh logic:
  - [x] Refresh at day boundary (midnight)
  - [x] Clear stale entries (older than yesterday)
- [x] No fallback mechanisms - propagate API failures per financial integrity
      rules

### Implementation Details

**Module Structure:**

- Created `src/schwab/market_hours_cache.rs` with proper visibility
  (`pub(crate)`)
- Updated `src/schwab/mod.rs` to export the new `MarketHoursCache` type
- Uses `tokio::sync::RwLock` for async thread-safe access (different from
  SymbolCache which uses std::sync::RwLock)

**Cache Design:**

```rust
pub(crate) struct MarketHoursCache {
    cache: RwLock<HashMap<(String, NaiveDate), MarketHours>>,
}
```

**Key Methods Implemented:**

- `get_or_fetch()` - Check cache first, fetch from API if missing/expired
- `get_current_status()` - Return current market status quickly
- `get_next_transition()` - Return next market transition time (open/close)
- `refresh_cache()` - Clear stale entries and pre-fetch today/tomorrow
- `clear_stale_entries()` - Remove entries older than yesterday

**Thread Safety & Performance:**

- Uses `tokio::sync::RwLock` for async operations
- Cache key format: `(market_id: String, date: NaiveDate)`
- Minimal memory footprint - only caches today and tomorrow's hours
- Proper lock scope management with explicit `drop()` calls to minimize
  contention

**Error Handling:**

- All API failures propagate without fallback values (per financial integrity
  rules)
- Uses existing `SchwabError` types for consistent error handling
- No silent failures or default values that could corrupt financial data

**Test Coverage:**

- 7 comprehensive tests covering:
  - Cache hit/miss scenarios
  - Thread-safe concurrent access patterns
  - API error propagation
  - Stale entry cleanup logic
  - Current status determination
  - Mock server integration
- All tests passing with proper business logic validation

**Code Quality:**

- Follows CLAUDE.md guidelines for functional programming patterns
- Uses minimal public API surface (`pub(crate)` visibility)
- Proper resource management with explicit lock dropping
- No unwrap patterns - all errors propagate explicitly

### Success Criteria

- ✅ Thread-safe caching with minimal memory footprint
- ✅ API failures propagate with proper error types
- ✅ No silent failures or default values
- ✅ Cache refresh is automatic and efficient
- ✅ Comprehensive test coverage
- ✅ All clippy lints pass
- ✅ Ready for integration in Task 4 (Trading Hours Controller)

## Task 4: Create Trading Hours Controller ✅ COMPLETED

### Problem Summary

Need a controller that determines when the bot should run based on market hours.

### Implementation Checklist

- [x] Create `src/trading_hours_controller.rs` module at root level
- [x] Hardcode configuration as constants:
  ```rust
  const MARKET_OPEN_BUFFER_MINUTES: i64 = 5;
  const MARKET_CLOSE_BUFFER_MINUTES: i64 = 5;
  const MARKET_ID: &str = "equity";
  ```
- [x] Implement `TradingHoursController` struct:
  - [x] `should_bot_run()` - determines if bot should be running now
  - [x] `wait_until_market_open()` - async wait until market opens
  - [x] `time_until_market_close()` - duration until market closes
  - [x] Hold `Arc<MarketHoursCache>` for shared access
- [x] Use proper state modeling - no string status fields
- [x] Add comprehensive logging with tracing crate
- [x] Keep all methods private except those needed by lib.rs

### Implementation Details

**Module Structure:**

- Created `src/trading_hours_controller.rs` at root level as specified
- Updated `src/lib.rs` to include the new module
- Used `pub(crate)` visibility for controlled access from lib.rs

**Configuration Constants:**

```rust
const MARKET_OPEN_BUFFER_MINUTES: i64 = 5;
const MARKET_CLOSE_BUFFER_MINUTES: i64 = 5;
const MARKET_ID: &str = "equity";
```

**TradingHoursController Implementation:**

```rust
pub(crate) struct TradingHoursController {
    cache: Arc<MarketHoursCache>,
    env: SchwabAuthEnv,
    pool: Arc<SqlitePool>,
}
```

**Key Methods Implemented:**

- `new()` - Constructor that takes shared cache, environment, and database pool
- `should_bot_run()` - Determines if bot should be running now with buffer
  logic:
  - Returns `true` if market is currently open
  - Returns `true` if market is closed but within 5-minute buffer before open
  - Returns `false` otherwise
- `wait_until_market_open()` - Async method that waits until market opens (with
  buffer):
  - Continuously checks market status
  - Calculates sleep duration until market open minus buffer
  - Provides informative logging for long waits and debug logging for short
    waits
  - Has fallback 1-minute sleep if unable to determine next open time
- `time_until_market_close()` - Returns duration until market closes (with
  buffer):
  - Returns `None` if market is already closed
  - Returns `Some(Duration)` with time until market close plus 5-minute buffer
  - Used for `tokio::select!` timeout in main bot loop

**Buffer Logic:**

- Bot starts 5 minutes before market opens (including the buffer in "should run"
  determination)
- Bot stops 5 minutes after market closes (buffer included in close time
  calculation)
- All buffer times are hardcoded as constants per requirements

**Error Handling:**

- All API failures are propagated without fallback values (financial integrity
  rules)
- Uses existing `SchwabError` types for consistency
- No `unwrap` or `unwrap_or` patterns - explicit error handling throughout

**Logging Integration:**

- Uses `tracing` crate for structured logging
- `info!` level for important state transitions (market open/close events)
- `debug!` level for routine status checks and short waits
- `warn!` level for unexpected conditions (unable to determine next open time)

**Thread Safety:**

- Uses `Arc<MarketHoursCache>` for shared, thread-safe cache access
- Uses `Arc<SqlitePool>` for shared database access
- All methods are async-safe and can be called concurrently

**Test Coverage:**

Comprehensive tests covering:

- `test_should_bot_run_market_open()` - Behavior when market is currently open
- `test_should_bot_run_market_closed_outside_buffer()` - Behavior when market is
  closed and outside buffer
- `test_should_bot_run_within_buffer_time()` - Behavior when market is closed
  but within 5-minute buffer
- `test_time_until_market_close_open_market()` - Duration calculation when
  market is open
- `test_time_until_market_close_closed_market()` - Behavior when market is
  closed
- `test_buffer_time_constants()` - Verification of hardcoded constants
- `test_api_error_propagation()` - Error handling when API calls fail

**Code Quality:**

- All tests pass (7 passing tests)
- Follows CLAUDE.md guidelines:
  - Minimal public API surface (`pub(crate)` only)
  - No unwrap patterns - explicit error handling
  - Proper state modeling with enums (leverages existing `MarketStatus`)
  - Functional programming patterns where appropriate
  - Clear separation of concerns

### Success Criteria

- ✅ Controller correctly determines when bot should run
- ✅ Respects hardcoded buffer times (5 minutes before open, 5 minutes after
  close)
- ✅ Clear state transitions with appropriate logging levels
- ✅ Thread-safe shared cache access via `Arc<MarketHoursCache>`
- ✅ Minimal public API surface (all methods `pub(crate)`)
- ✅ Comprehensive test coverage with business logic validation
- ✅ Ready for integration in Task 5

## Task 5: Integrate Trading Hours Controller into Main Bot

### Problem Summary

The main bot runner needs modification to start/stop the entire bot flow based
on market hours. The bot will completely shut down when market closes and
restart when market opens.

### Implementation Checklist

- [ ] Modify `src/lib.rs` `run()` function:
  - [ ] Add optional flag to enable/disable market hours check (for testing):
    ```rust
    // Check env var but don't expose in Env struct
    let check_market_hours = std::env::var("DISABLE_MARKET_HOURS_CHECK")
        .map(|v| v != "true")
        .unwrap_or(true);
    ```
  - [ ] Initialize `MarketHoursCache` and `TradingHoursController` if enabled
  - [ ] Wrap entire bot flow in market hours check:
    ```rust
    loop {
        // Wait until market opens
        if check_market_hours && !controller.should_bot_run() {
            info!("Market closed, waiting until market opens...");
            controller.wait_until_market_open().await?;
        }
        
        // Run the entire bot flow (existing code)
        // - Token validation
        // - WebSocket connection
        // - Backfill
        // - Live monitoring
        
        // Run until market closes
        if check_market_hours {
            let close_time = controller.time_until_market_close();
            tokio::select! {
                result = run_bot() => { /* handle result */ }
                _ = tokio::time::sleep(close_time) => {
                    info!("Market closing, shutting down bot");
                    // Graceful shutdown
                }
            }
        } else {
            run_bot().await?;
        }
    }
    ```
- [ ] Update retry logic to include market hours:
  - [ ] When market is closed, wait for next open
  - [ ] No retries during closed hours
- [ ] Ensure clean shutdown/startup:
  - [ ] All connections closed properly
  - [ ] All tasks cancelled
  - [ ] Fresh start each market open

### Key Behavior

The bot will:

1. Check if market is open before starting
2. If closed, wait until market opens
3. Start entire bot flow (backfill → live monitoring)
4. Run until market closes
5. Shut down completely
6. Repeat from step 1

### Success Criteria

- Bot only runs during market hours (±buffer)
- Complete shutdown when market closes
- Fresh start with backfill each market open
- No orphaned tasks or connections
- Clear logging of start/stop cycles
- Can bypass for testing with env var

## Implementation Order

1. **Task 1**: Create Market Hours API Client Module (foundation)
2. **Task 2**: Add CLI Command (testing tool)
3. **Task 3**: Create Market Hours Cache Module (performance)
4. **Task 4**: Create Trading Hours Controller (control logic)
5. **Task 5**: Integrate into Main Bot (main integration)

## Testing Strategy

### Unit Tests

Per CLAUDE.md guidelines, tests must verify business logic:

- Market hours parsing with various API responses
- Timezone conversion edge cases (DST transitions)
- Cache expiration and refresh logic
- Controller state transitions
- Thread-safe cache operations

### Integration Tests

- Mock Schwab API responses for various scenarios
- Test weekend/holiday behavior (API returns is_open=false)
- Verify clean shutdown at market close
- Verify fresh start with backfill at market open
- Test bypass flag works correctly

### Manual Testing

- Use CLI command to verify market status
- Monitor logs during market open/close transitions
- Verify bot stops completely on market close
- Verify bot starts fresh on market open
- Test over weekend (bot should wait until Monday)
- Test with `DISABLE_MARKET_HOURS_CHECK=true` for 24/7 operation

## Configuration Summary

Hardcoded configuration (not exposed as env vars):

```rust
// In trading_hours_controller.rs
const MARKET_OPEN_BUFFER_MINUTES: i64 = 5;
const MARKET_CLOSE_BUFFER_MINUTES: i64 = 5;  
const MARKET_ID: &str = "equity";
```

Testing override (not in Env struct):

```bash
# Only for testing - disables market hours checking
DISABLE_MARKET_HOURS_CHECK=true
```

Note: No extended hours support - only regular market hours (9:30 AM - 4:00 PM
ET)

## Module Organization

Following CLAUDE.md visibility guidelines (most restrictive possible):

```
src/
├── schwab/
│   ├── mod.rs                      # pub(crate) exports only what's needed
│   ├── market_hours.rs             # Types and functions, minimal pub visibility
│   └── market_hours_cache.rs       # Cache implementation, mostly private
├── trading_hours_controller.rs     # Controller logic, minimal pub surface
├── lib.rs                          # (modified) Main bot runner
└── cli.rs                          # (modified) CLI commands
```

## Code Style Compliance

Following CLAUDE.md guidelines:

- Use ADTs and enums instead of string status fields
- Explicit error handling with typed errors (no unwrap_or)
- Functional programming patterns with iterators
- Thread-safe types with Arc/RwLock for shared state
- Meaningful tests that verify business logic
- Most restrictive visibility (private by default, pub(crate) when needed)
- Early returns to avoid deep nesting
- Clear separation of concerns

## Critical Design Decisions

1. **Complete Start/Stop**: Entire bot flow starts/stops based on market hours
2. **Idempotent Design**: Each start runs backfill ensuring no missed trades
3. **No Partial Operation**: Simplifies implementation - everything runs or
   nothing runs
4. **Clean State**: Fresh start each market open, no state carried over
5. **No Fallbacks**: API must be available - fail fast if unavailable
6. **Minimal Configuration**: Hardcode sensible defaults, only testing override

## Risk Considerations

1. **API Dependency**: System requires Schwab API availability at market open
2. **Timezone Handling**: Critical to handle DST transitions correctly
3. **Startup Time**: Must complete backfill quickly after market opens
4. **Shutdown Timing**: Must complete in-flight trades before market close
5. **Token Expiry**: Tokens expire after 30 minutes when bot is stopped

## Success Metrics

- Bot starts within 5 minutes of market open
- Bot stops within 5 minutes of market close
- Backfill completes successfully each market open
- No trades attempted outside market hours
- Clean shutdown with no orphaned resources
- Clear audit trail in logs showing start/stop cycles

## Notes

- Regular market hours only: 9:30 AM - 4:00 PM ET
- Holidays handled by API returning is_open=false
- Use chrono-tz for timezone handling
- Cache is minimal: only today and tomorrow
- Complete shutdown simplifies state management
- Idempotent design ensures no missed trades
- Configuration is hardcoded for simplicity
