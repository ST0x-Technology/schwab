# st0x peg management system

An arbitrage bot for tokenized equities that bridges onchain Raindex orderbook
trades with Charles Schwab brokerage executions.

## Overview

Schwab monitors onchain tokenized equity trades and executes offsetting trades
on Charles Schwab to maintain market-neutral positions while capturing spread
differentials. The bot helps establish price discovery in early-stage onchain
equity markets by arbitraging discrepancies between onchain prices and
traditional market prices.

## Prerequisites

- Nix with flakes enabled
- Charles Schwab brokerage account with API access
- Ethereum node with WebSocket access

## Quick Start

### 1. Charles Schwab Setup

First, set up a Charles Schwab account. If you are based outside of the US,
register with Charles Schwab International.

Once your trading account is established, navigate to the
[Schwab Developer Portal](https://developer.schwab.com/).

Register a new account on this site using the same details as your trading
account. After completing registration, you will see three setup options:
Individual, Company, or Join a Company. Select the option to set up as an
individual.

Next, proceed to the API Products section and choose "Individual Developers".
Click on "Trader API" and request access. In the request make sure you add your
Charles Schwab account number.

Charles Schwab will then process your request, which typically takes 3-5 days.
During this period, your developer account will be linked with your trading
account.

### 2. Configuration

Create a `.env` file in the project root and set the database URL:

```bash
DATABASE_URL=sqlite:schwab.db
```

For all other environment variables, refer to `.env.example` and configure as
needed.

### 3. Database Setup

```bash
# Create database and run migrations
sqlx db create
sqlx migrate run
```

### 4. Authentication

Authenticate with Charles Schwab (one-time setup):

```bash
cargo run --bin cli -- auth
```

Follow the OAuth flow to obtain and store your access and refresh tokens.

### 5. Run the Bot

```bash
cargo run --bin server
```

## Development

### With Nix

Enter the development shell with all dependencies:

```bash
nix develop
```

### Nix Scripts

The flake provides the following utility scripts:

```bash
# Build Solidity artifacts for the orderbook interface
nix run .#prepSolArtifacts

# Generate test coverage report (outputs to HTML)
nix run .#checkTestCoverage
```

### Building

```bash
cargo build
```

### Testing

```bash
# Run all tests
cargo test -q

# Run with coverage (or use nix run .#checkTestCoverage)
cargo-tarpaulin --skip-clean --out Html
```

### Code Quality

```bash
# Format code
cargo fmt

# Run linter
cargo clippy --all-targets --all-features -- -D clippy::all

# Run static analysis
rainix-rs-static
```

## Project Structure

- `src/lib.rs` - Main event loop and orchestration
- `src/bin/server.rs` - Arbitrage bot server binary
- `src/bin/reporter.rs` - P&L reporter binary
- `src/bin/cli.rs` - Command-line interface for operations
- `src/trade/` - Trade conversion and validation logic
- `src/schwab.rs` - Charles Schwab API integration and OAuth
- `src/reporter/` - P&L calculation and FIFO inventory logic
- `src/symbol/` - Token symbol caching
- `migrations/` - SQLite database schema
- `AGENTS.md` - Development guidelines for AI assistance
- `SPEC.md` - Full technical specification

## P&L Reporter

The reporter calculates realized profit/loss using FIFO (First-In-First-Out)
accounting. It processes all trades (onchain and offchain) and maintains
performance metrics in the `metrics_pnl` table for Grafana visualization.

### How It Works

- **FIFO Accounting**: Oldest position lots are consumed first when closing
  positions
- **In-Memory State**: FIFO inventory rebuilt on startup by replaying all trades
- **Checkpoint**: Uses MAX(timestamp) from metrics_pnl to resume processing new
  trades
- **All Trades Tracked**: Both position-increasing and position-reducing trades
  recorded

### Running Locally

```bash
# Run reporter
cargo run --bin reporter
```

### Metrics Table Schema

Every trade gets a row in `metrics_pnl`:

- **realized_pnl**: NULL for position increases, value for position decreases
- **cumulative_pnl**: Running total of realized P&L for this symbol
- **net_position_after**: Current position after trade (positive=long,
  negative=short)

### Example

```text
Step 1: BUY 100 @ $10.00
  - Opens lot: 100 shares @ $10.00 cost basis
  - realized_pnl: NULL
  - net_position_after: 100

Step 2: SELL 60 @ $11.00
  - Consumes 60 from oldest lot (FIFO)
  - realized_pnl: (11.00 - 10.00) * 60 = $60.00
  - cumulative_pnl: $60.00
  - net_position_after: 40
```

## Architecture

The bot uses an event-driven async architecture:

1. WebSocket monitors blockchain events (ClearV2, TakeOrderV2)
2. Each event spawns independent async task for processing
3. Events are deduplicated using (tx_hash, log_index) keys
4. Onchain trades accumulate per symbol in database
5. When accumulated position reaches ≥1.0 shares, execute on Schwab
6. Complete audit trail links onchain trades to offchain executions

See [SPEC.md](SPEC.md) for detailed architecture and operational flows.
