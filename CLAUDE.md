# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

Like all other AI agents working in this repo, Claude Code MUST obey @AGENTS.md

## P&L Reporter

### Architecture

**Reporter Binary (`src/bin/reporter.rs`)**

- Processes trades to calculate FIFO P&L
- Runs independently in Docker container
- Shares SQLite database via mounted volume

**Core FIFO Logic (`src/reporter/pnl.rs`)**

- `FifoInventory`: Manages per-symbol lot queues
- In-memory state rebuilt on startup by replaying trades
- Uses rust_decimal for financial precision
- Handles position increases, decreases, and reversals

**Database Schema:**

- `metrics_pnl`: Single table for metrics and checkpoint
  - Every trade gets a row (position-increasing and position-reducing)
  - `realized_pnl` NULL for increases, value for decreases
  - UNIQUE constraint on (trade_type, trade_id) prevents duplicates
  - MAX(timestamp) serves as processing checkpoint

### Precision Trade-off

- Internal calculations use `Decimal` for precision
- `metrics_pnl` uses REAL (f64) for Grafana compatibility
- Conversion to f64 has acceptable precision loss for analytics
- Source of truth (`onchain_trades`, `schwab_executions`) maintains full
  precision
