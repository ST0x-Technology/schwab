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

## Task 3: Create Market Hours Cache Module

### Problem Summary

To avoid excessive API calls, we need to cache market hours in memory similar to
the existing `SymbolCache`.

### Implementation Checklist

- [ ] Create `src/schwab/market_hours_cache.rs` module following SymbolCache
      pattern
- [ ] Implement `MarketHoursCache` struct:
  - [ ] Use `tokio::sync::RwLock` for thread-safe access
  - [ ] Store `HashMap<(String, NaiveDate), MarketHours>`
  - [ ] Cache only today's and tomorrow's hours (minimal memory)
- [ ] Implement methods following functional programming patterns:
  - [ ] `get_or_fetch()` - check cache first, fetch if missing
  - [ ] `get_current_status()` - return current market status
  - [ ] `get_next_transition()` - return next open/close time
- [ ] Cache refresh logic:
  - [ ] Refresh at day boundary (midnight)
  - [ ] Clear stale entries (older than yesterday)
- [ ] No fallback mechanisms - propagate API failures per financial integrity
      rules

### Success Criteria

- Thread-safe caching with minimal memory footprint
- API failures propagate with proper error types
- No silent failures or default values
- Cache refresh is automatic and efficient

## Task 4: Create Trading Hours Controller

### Problem Summary

Need a controller that determines when the bot should run based on market hours.

### Implementation Checklist

- [ ] Create `src/trading_hours_controller.rs` module at root level
- [ ] Hardcode configuration as constants:
  ```rust
  const MARKET_OPEN_BUFFER_MINUTES: i64 = 5;
  const MARKET_CLOSE_BUFFER_MINUTES: i64 = 5;
  const MARKET_ID: &str = "equity";
  ```
- [ ] Implement `TradingHoursController` struct:
  - [ ] `should_bot_run()` - determines if bot should be running now
  - [ ] `wait_until_market_open()` - async wait until market opens
  - [ ] `time_until_market_close()` - duration until market closes
  - [ ] Hold `Arc<MarketHoursCache>` for shared access
- [ ] Use proper state modeling - no string status fields
- [ ] Add comprehensive logging with tracing crate
- [ ] Keep all methods private except those needed by lib.rs

### Success Criteria

- Controller correctly determines when bot should run
- Respects hardcoded buffer times
- Clear state transitions with logging
- Thread-safe shared cache access
- Minimal public API surface

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
