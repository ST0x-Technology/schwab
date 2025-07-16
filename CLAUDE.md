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
- `cargo-clippy` - Run linter
- `cargo fmt` - Format code
- `cargo-tarpaulin --skip-clean --out Html` - Generate test coverage report
- `bacon` - Watch mode for continuous compilation

### Mock Server
- `npm run mock` - Start mock Charles Schwab API server on port 4020

### Nix Development Environment
- `nix develop` - Enter development shell with all dependencies
- `nix run .#prepSolArtifacts` - Build Solidity artifacts for orderbook interface
- `nix run .#checkTestCoverage` - Generate test coverage report

## Architecture Overview

### Core Event Processing Flow

**Main Event Loop (`src/lib.rs:35-63`)**
- Monitors two concurrent WebSocket event streams: `ClearV2` and `TakeOrderV2` from the Raindex orderbook
- Uses `tokio::select!` to handle events from either stream without blocking
- Converts blockchain events to structured `Trade` objects for processing

**Trade Conversion Logic (`src/trade/mod.rs`)**
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

### Key Architectural Decisions

- **Event-Driven Architecture**: Each trade spawns independent async task for maximum throughput
- **SQLite Persistence**: Embedded database for trade tracking and authentication tokens
- **Symbol Suffix Convention**: Tokenized equities use "s1" suffix to distinguish from base assets
- **Price Direction Logic**: Onchain buy = offchain sell (and vice versa) to maintain market-neutral positions
- **Comprehensive Error Handling**: Custom error types (`TradeConversionError`, `SchwabAuthError`) with proper propagation
- **Idiomatic Functional Programming**: Prefer iterator-based functional programming patterns over imperative loops unless it increases complexity

### Testing Strategy

- **Mock Blockchain Interactions**: Uses `alloy::providers::mock::Asserter` for deterministic testing
- **HTTP API Mocking**: `httpmock` crate for Charles Schwab API testing
- **Database Isolation**: In-memory SQLite databases for test isolation
- **Edge Case Coverage**: Comprehensive error scenario testing for trade conversion logic

### Known V1 Limitations

- **After-Hours Trading Gap**: Pyth oracle operates when traditional markets are closed
- **Manual Rebalancing**: Inventory management via st0x bridge requires manual intervention
- **Missed Trade Risk**: Bot downtime or API failures can create unhedged exposure
- **No Circuit Breakers**: Limited risk management controls in initial implementation
