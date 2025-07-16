# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a hybrid Rust and Solidity project that implements a trading system connecting blockchain transactions to Schwab brokerage API. The system:

1. **Monitors blockchain events** from an Order Book contract using Alloy (Rust Ethereum library)
2. **Processes trades** by parsing ClearV2 and TakeOrderV2 events
3. **Maps onchain tokens** to traditional stock symbols (e.g., "FOOs1" token → "FOO" stock)
4. **Interfaces with Schwab API** for OAuth authentication and order execution
5. **Stores data** in SQLite database for persistence

## Build and Development Commands

### Rust Development
```bash
# Build the project
cargo build

# Run tests
cargo test

# Run integration tests
cargo test --test integration

# Run with coverage report
cargo-tarpaulin --skip-clean --out Html
# or use the nix task:
nix run .#checkTestCoverage

# Run the main application
cargo run --bin main

# Run the auth utility
cargo run --bin auth
```

### Solidity Development
```bash
# Build Solidity contracts (via nix)
nix run .#prepSolArtifacts

# Or manually:
cd lib/rain.orderbook.interface/ && forge build
cd lib/forge-std/ && forge build
```

### Mock Server
```bash
# Start mock Schwab API server (for development/testing)
npm run mock
```

### Database Management
```bash
# Run database migrations
sqlx migrate run

# Set up database URL
export DATABASE_URL="sqlite:schwab.db"
```

## Code Architecture

### Core Components

1. **`src/lib.rs`** - Main application entry point and event loop
   - Sets up WebSocket connection to blockchain RPC
   - Filters for OrderBook events (ClearV2, TakeOrderV2)
   - Orchestrates trade processing pipeline

2. **`src/schwab.rs`** - Schwab API integration
   - OAuth 2.0 authentication flow
   - Token storage and refresh logic
   - Future: order execution API calls

3. **`src/trade/`** - Trade processing logic
   - `mod.rs` - Core trade types and conversion logic
   - `clear.rs` - Handles ClearV2 events (order clearing)
   - `take_order.rs` - Handles TakeOrderV2 events (order taking)

4. **`src/symbol_cache.rs`** - Token symbol caching
   - Caches ERC20 symbol() calls to avoid repeated RPC requests
   - Maps contract addresses to human-readable symbols

5. **`src/bindings.rs`** - Generated Solidity bindings
   - Auto-generated from OrderBook and ERC20 interfaces
   - Provides type-safe contract interaction

### Key Data Flow

1. **Event Monitoring**: WebSocket listens for blockchain events
2. **Event Parsing**: Extract order details and fill information
3. **Symbol Resolution**: Map contract addresses to symbols (cached)
4. **Trade Validation**: Ensure valid USDC ↔ stock token pairs
5. **Schwab Mapping**: Convert onchain data to Schwab order format
6. **Database Storage**: Persist trade records (TODO: implement)

### Configuration

The application uses environment variables and CLI arguments (via clap):

- `DATABASE_URL` - SQLite database connection string
- `APP_KEY` / `APP_SECRET` - Schwab API credentials
- `REDIRECT_URI` - OAuth redirect URI (default: https://127.0.0.1)
- `BASE_URL` - Schwab API base URL (default: https://api.schwabapi.com)
- `WS_RPC_URL` - Blockchain WebSocket RPC endpoint
- `ORDERBOOK` - OrderBook contract address
- `ORDER_HASH` - Specific order hash to monitor

### Token Symbol Convention

The system expects a specific token naming pattern:
- Onchain tokens end with "s1" suffix (e.g., "FOOs1", "BARs1")
- Traditional stock symbols are derived by removing the "s1" suffix
- USDC is used as the base currency for price calculations

### Database Schema

See `migrations/20250703115746_trades.sql` for the complete schema:
- `trades` table stores transaction details and Schwab order information
- `schwab_auth` table stores OAuth tokens with expiration tracking

## Testing

The project includes comprehensive unit tests with mocked blockchain providers:
- Mock Alloy providers for testing contract interactions
- Test utilities in `src/test_utils.rs`
- Integration tests in `tests/integration.rs`

## Development Environment

This project uses Nix for reproducible development environments:
- `flake.nix` defines the development shell
- Includes tools: sqlx-cli, bacon, cargo-tarpaulin
- Solidity build tools via rainix

## Security Considerations

- OAuth tokens are stored in SQLite with expiration tracking
- Never commit API keys or secrets to version control
- Use environment variables for sensitive configuration