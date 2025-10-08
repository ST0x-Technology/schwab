# FIFO P&L Reporter Implementation Plan

## Overview

Implement a FIFO P&L reporter as a separate Rust binary that processes trades
and writes performance metrics to a single `metrics_pnl` table for Grafana
visualization. The reporter maintains in-memory FIFO inventory state and can
rebuild it on restart by replaying all trades.

**Key Design Decisions:**

1. **Single Table**: Only `metrics_pnl` - serves both as metrics storage and
   processing checkpoint
2. **All Trades Tracked**: Every trade (position-increasing and
   position-reducing) gets a row
3. **Timestamp Checkpoint**: Use `MAX(timestamp)` from metrics_pnl to resume
   processing
4. **In-Memory FIFO**: Rebuild state on startup by replaying all trades (fast,
   no writes)
5. **Flake.nix Scripts**: Development tools follow existing patterns in
   flake.nix
6. **Data Focus**: Get metrics into database; Grafana dashboards come later

---

## Task 1. Database Schema Design

Design single `metrics_pnl` table that serves both as Grafana metrics and
processing checkpoint.

### Rationale

Every trade (both position-increasing and position-reducing) needs a row in
metrics_pnl. This allows us to:

- Track all position changes over time
- Use MAX(timestamp) as checkpoint for resuming processing
- Filter by realized_pnl IS NOT NULL to see only P&L realization events
- Avoid needing separate checkpoint or inventory tables

### Subtasks

- [x] Design `metrics_pnl` table schema with all necessary columns
- [x] Add constraints to ensure data integrity (positive quantities, valid trade
      types)
- [x] Add indexes for Grafana queries (symbol + timestamp, symbol alone)
- [x] Add UNIQUE constraint on (trade_type, trade_id) to prevent duplicate
      processing
- [x] Write migration file `migrations/20251002210824_pnl_metrics.sql`
- [x] Ensure schema supports rust_decimal precision for financial data

### Table Schema

```sql
CREATE TABLE metrics_pnl (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  symbol TEXT NOT NULL CHECK (symbol != ''),
  timestamp TIMESTAMP NOT NULL,
  trade_type TEXT NOT NULL CHECK (trade_type IN ('ONCHAIN', 'OFFCHAIN')),
  trade_id INTEGER NOT NULL,
  trade_direction TEXT NOT NULL CHECK (trade_direction IN ('BUY', 'SELL')),
  quantity REAL NOT NULL CHECK (quantity > 0),
  price_per_share REAL NOT NULL CHECK (price_per_share > 0),
  realized_pnl REAL,  -- NULL for position-increasing trades, value for position-reducing
  cumulative_pnl REAL NOT NULL,  -- Running total for this symbol
  net_position_after REAL NOT NULL,  -- Position after this trade (can be negative for short)
  UNIQUE (trade_type, trade_id)  -- Prevents duplicate processing
);

CREATE INDEX idx_metrics_pnl_symbol_timestamp ON metrics_pnl(symbol, timestamp);
CREATE INDEX idx_metrics_pnl_symbol ON metrics_pnl(symbol);
CREATE INDEX idx_metrics_pnl_timestamp ON metrics_pnl(timestamp);
```

**Column Explanations:**

- `realized_pnl`: NULL when trade increases position (opens new lot), has value
  when trade decreases position (consumes lots)
- `cumulative_pnl`: Running total of all realized P&L for this symbol up to this
  trade
- `net_position_after`: Positive = long position, negative = short position,
  zero = flat
- `UNIQUE (trade_type, trade_id)`: Ensures each trade processed exactly once

---

## Task 2. Core FIFO P&L Logic

Implement FIFO algorithm with rust_decimal for financial precision.

### Rationale

The FIFO algorithm manages an in-memory queue of position lots per symbol. When
trades come in:

- **Position increase**: Add new lot to end of queue
- **Position decrease**: Consume oldest lots first (FIFO), calculate realized
  P&L
- **Position reversal**: Close all lots in one direction, then open new lots in
  opposite direction

Using rust_decimal prevents floating-point precision errors. All calculations
must use explicit error handling (no silent data corruption).

### Subtasks

- [x] Create `src/reporter/pnl.rs` module for FIFO logic
- [x] Define core types: `InventoryLot`, `TradeType`, `PnlResult`
- [x] Reuse existing `schwab::Direction` enum (no duplicates)
- [x] Implement `FifoInventory` struct with VecDeque for lot storage
- [x] Implement `add_lot()` method for position-increasing trades
- [x] Implement `consume_lots()` method with FIFO logic and P&L calculation
- [x] Implement `process_trade()` method to determine increase vs decrease
- [x] Handle position reversals (long→short, short→long)
- [x] Add comprehensive error types
- [x] Write unit tests for all FIFO scenarios

### Core Types

```rust
use crate::schwab::Direction;  // USE EXISTING Direction ENUM - NO DUPLICATES
use rust_decimal::Decimal;
use std::collections::VecDeque;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TradeType {
    Onchain,
    Offchain,
}

#[derive(Debug, Clone)]
struct InventoryLot {
    quantity_remaining: Decimal,
    cost_basis_per_share: Decimal,
    direction: Direction,  // Direction::Buy or Direction::Sell - REUSE existing type
}

pub struct FifoInventory {
    symbol: String,
    lots: VecDeque<InventoryLot>,
    cumulative_pnl: Decimal,
}

pub struct PnlResult {
    pub realized_pnl: Option<Decimal>,  // None if position increased, Some if decreased
    pub cumulative_pnl: Decimal,
    pub net_position_after: Decimal,
}
```

### FIFO Algorithm

**IMPORTANT: Uses existing `schwab::Direction` enum (Buy/Sell) - NO new
direction types**

```rust
impl FifoInventory {
    pub fn process_trade(
        &mut self,
        quantity: Decimal,
        price_per_share: Decimal,
        direction: Direction,  // Direction::Buy or Direction::Sell
    ) -> Result<PnlResult, PnlError> {
        // Determine if trade increases or decreases position
        match self.current_direction() {
            None => {
                // Flat position -> opens new position
                self.add_lot(quantity, price_per_share, direction);
                Ok(PnlResult {
                    realized_pnl: None,
                    cumulative_pnl: self.cumulative_pnl,
                    net_position_after: self.net_position(),
                })
            }
            Some(current) if current == direction => {
                // Same direction -> increases position
                self.add_lot(quantity, price_per_share, direction);
                Ok(PnlResult {
                    realized_pnl: None,
                    cumulative_pnl: self.cumulative_pnl,
                    net_position_after: self.net_position(),
                })
            }
            Some(_) => {
                // Opposite direction -> decreases position (may reverse)
                let pnl = self.consume_lots(quantity, price_per_share, direction)?;
                self.cumulative_pnl = self.cumulative_pnl.checked_add(pnl)
                    .ok_or(PnlError::ArithmeticOverflow)?;
                Ok(PnlResult {
                    realized_pnl: Some(pnl),
                    cumulative_pnl: self.cumulative_pnl,
                    net_position_after: self.net_position(),
                })
            }
        }
    }

    fn consume_lots(
        &mut self,
        quantity: Decimal,
        execution_price: Decimal,
        direction: Direction,
    ) -> Result<Decimal, PnlError> {
        // Use try_fold iterator pattern - no imperative loops
        let (total_pnl, remaining) = self.lots.iter_mut().try_fold(
            (Decimal::ZERO, quantity),
            |(pnl_acc, qty_remaining), lot| {
                if qty_remaining == Decimal::ZERO {
                    return Ok((pnl_acc, qty_remaining));
                }

                let consumed = qty_remaining.min(lot.quantity_remaining);

                let pnl = match lot.direction {
                    Direction::Buy => (execution_price - lot.cost_basis_per_share)
                        .checked_mul(consumed)
                        .ok_or(PnlError::ArithmeticOverflow)?,
                    Direction::Sell => (lot.cost_basis_per_share - execution_price)
                        .checked_mul(consumed)
                        .ok_or(PnlError::ArithmeticOverflow)?,
                };

                lot.quantity_remaining = lot.quantity_remaining
                    .checked_sub(consumed)
                    .ok_or(PnlError::ArithmeticOverflow)?;

                let new_pnl = pnl_acc.checked_add(pnl)
                    .ok_or(PnlError::ArithmeticOverflow)?;
                let new_remaining = qty_remaining.checked_sub(consumed)
                    .ok_or(PnlError::ArithmeticOverflow)?;

                Ok((new_pnl, new_remaining))
            },
        )?;

        self.lots.retain(|lot| lot.quantity_remaining > Decimal::ZERO);

        if remaining > Decimal::ZERO {
            self.add_lot(remaining, execution_price, direction);
        }

        Ok(total_pnl)
    }

    fn add_lot(&mut self, quantity: Decimal, price: Decimal, direction: Direction) {
        self.lots.push_back(InventoryLot {
            quantity_remaining: quantity,
            cost_basis_per_share: price,
            direction,
        });
    }

    fn net_position(&self) -> Decimal {
        self.lots.iter().fold(Decimal::ZERO, |acc, lot| {
            match lot.direction {
                Direction::Buy => acc + lot.quantity_remaining,
                Direction::Sell => acc - lot.quantity_remaining,
            }
        })
    }

    fn current_direction(&self) -> Option<Direction> {
        match self.net_position().cmp(&Decimal::ZERO) {
            Ordering::Greater => Some(Direction::Buy),
            Ordering::Less => Some(Direction::Sell),
            Ordering::Equal => None,
        }
    }
}
```

### Unit Tests

- [ ] Test: Simple buy then sell (basic FIFO)
- [ ] Test: Multiple buys at different prices, then partial sell
- [ ] Test: Position reversal (long → short)
- [ ] Test: Short position P&L calculation
- [ ] Test: Example from requirements doc (7-step scenario)
- [ ] Test: Fractional share handling
- [ ] Test: Precision with rust_decimal (no floating point errors)

---

## Task 3. Reporter Implementation

Create `reporter` binary that loads trades, processes with FIFO, writes to
metrics_pnl.

### Rationale

The reporter runs independently in a loop:

1. Load checkpoint (MAX timestamp from metrics_pnl)
2. Rebuild in-memory FIFO state by replaying all trades from beginning
3. Process new trades (timestamp > checkpoint)
4. Write results to metrics_pnl in transaction
5. Sleep and repeat

Rebuilding FIFO state on each iteration is fast (no database writes during
replay) and ensures correctness if database is manually modified.

### Design Decisions

**Code Organization:**

- Module structure: `reporter::pnl` (reporter reports P&L, not P&L reports)
- FIFO logic in `src/reporter/pnl.rs`
- Reporter config and loop logic in `src/reporter/mod.rs`
- Binary `src/bin/reporter.rs` is minimal (matches pattern of other binaries)
- This enables comprehensive unit testing and code reuse

**Type System:**

- Created `Symbol` newtype in `src/symbol/mod.rs` for type safety
  - `pub(crate)` visibility following CLAUDE.md guidelines
  - Validated via `TryFrom<String>` and `TryFrom<&str>` (no empty symbols)
  - No unnecessary `new()` constructor - use TryFrom implementations
- Trade struct uses `r#type` (not `trade_type`) and `id` (not `trade_id`)
- Leverages existing `Direction::from_str()` instead of custom parsing
- Uses algebraic data types to make invalid states unrepresentable

**Functional Programming:**

- `rebuild_fifo_state()` uses `try_fold()` iterator pattern instead of
  imperative loops
- Trade loading uses iterator chains with `map()` and `collect()`
- Eliminated mutable variables where possible

**Database:**

- SQL formatted with newlines per column for readability
- All metric conversions centralized in `Trade::to_db_values()`
- Checkpoint uses `DateTime::UNIX_EPOCH` as default instead of special sentinel
  values
- **Precision Trade-off**: `metrics_pnl` uses REAL (f64) for Grafana
  compatibility
  - Internal calculations use `Decimal` for precision
  - Conversion to f64 for database storage has acceptable precision loss for
    analytics
  - Source of truth (`onchain_trades`, `schwab_executions`) maintains full
    precision
  - Design documented in CLAUDE.md and README.md

**Error Handling:**

- Never imports bare `Result` - always uses qualified `anyhow::Result`
- Explicit error context for all conversions
- Proper error propagation with `?` operator

### Subtasks

- [x] Create `src/bin/reporter.rs` binary entry point
- [x] Implement reporter initialization (DB pool, environment config)
- [x] Implement `load_checkpoint()` to get MAX timestamp from metrics_pnl
- [x] Implement `load_all_trades()` to fetch and merge onchain + offchain trades
- [x] Implement `rebuild_fifo_state()` to replay trades up to checkpoint
- [x] Implement `process_new_trades()` to process trades after checkpoint
- [x] Write metrics_pnl rows in database transactions
- [x] Handle graceful shutdown (SIGTERM/SIGINT)
- [x] Integration tests with in-memory SQLite

### Reporter Structure

```rust
// src/bin/reporter.rs (minimal entry point, matches other binaries)
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv_override().ok();
    let config = ReporterConfig::parse();
    setup_tracing(&config.log_level);

    let pool = config.get_sqlite_pool().await?;
    let interval = Duration::from_secs(config.reporter_processing_interval_secs);

    run_reporter_loop(pool, interval).await
}

// src/reporter/mod.rs (library code with config and logic)
#[derive(Debug, Clone)]
struct Trade {
    r#type: TradeType,
    id: i64,
    symbol: Symbol,
    quantity: Decimal,
    price_per_share: Decimal,
    direction: Direction,
    timestamp: DateTime<Utc>,
}

async fn process_iteration(pool: &SqlitePool) -> anyhow::Result<usize> {
    let checkpoint = load_checkpoint(pool).await?;
    let all_trades = load_all_trades(pool).await?;
    let mut inventories = rebuild_fifo_state(&all_trades, checkpoint)?;

    let new_trades: Vec<_> = all_trades
        .into_iter()
        .filter(|t| t.timestamp > checkpoint)
        .collect();

    for trade in &new_trades {
        process_and_persist_trade(pool, &mut inventories, trade).await?;
    }

    Ok(new_trades.len())
}

pub async fn run_reporter_loop(pool: SqlitePool, interval: Duration) -> anyhow::Result<()> {
    info!("Starting P&L reporter");
    sqlx::migrate!().run(&pool).await?;

    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                match result {
                    Ok(()) => info!("Shutdown signal received"),
                    Err(e) => error!("Error receiving shutdown signal: {e}"),
                }
                break;
            }
            () = tokio::time::sleep(interval) => {
                match process_iteration(&pool).await {
                    Ok(count) => info!("Processed {count} new trades"),
                    Err(e) => error!("Processing error: {e}"),
                }
            }
        }
    }

    info!("Reporter shutdown complete");
    Ok(())
}
```

---

## Task 4. Docker Compose Integration

Add reporter container to docker-compose.template.yaml.

### Rationale

The reporter runs alongside the arbitrage bot, sharing the SQLite database via
mounted volume. Both containers built from same Dockerfile.

### Subtasks

- [x] Update `docker-compose.template.yaml` to add reporter
- [x] Update `Dockerfile` to build reporter binary
- [x] Don't update `.env.example` with (use default
      `REPORTER_PROCESSING_INTERVAL_SECS`)
- [x] Ensure SQLite WAL mode is enabled for concurrent access
- [x] Test docker image builds locally
- [x] Update `docker-compose.template.yaml` to use `${DOCKER_IMAGE}` variable
- [x] Update `Dockerfile` to support `BUILD_PROFILE` arg (debug vs release)
- [x] Create `prepDockerCompose` script in `flake.nix` with `--prod` flag
- [x] Update `.github/workflows/deploy.yaml` to use `${DOCKER_IMAGE}` variable
- [x] Test locally without `--prod` flag (debug build, local paths)
- [x] Verify CI/CD configuration matches script behavior

### Implementation Details

#### `prepDockerCompose` Script Behavior

**Default (no flag) - Local/Debug Mode:**

- Builds Docker image locally with debug profile (faster builds)
- Sets `DOCKER_IMAGE=schwarbot:local`
- Sets `DATA_VOLUME_PATH=./data`
- Generates `docker-compose.yaml` from template using `envsubst`
- Runs `docker build` with `--build-arg BUILD_PROFILE=debug`

**With `--prod` flag - Production/CI Mode:**

- Uses pre-built registry images
- Sets
  `DOCKER_IMAGE=registry.digitalocean.com/${REGISTRY_NAME}/schwarbot:${SHORT_SHA}`
- Sets `DATA_VOLUME_PATH=${DATA_VOLUME_PATH}` (from environment)
- Generates `docker-compose.yaml` from template using `envsubst`
- Does NOT build (assumes image already exists in registry)

#### Docker Compose Template Changes

```yaml
services:
  schwarbot:
    image: ${DOCKER_IMAGE}
    # ... rest of config

  reporter:
    image: ${DOCKER_IMAGE}
    # ... rest of config
```

#### Dockerfile Changes

Add `BUILD_PROFILE` argument to support both debug and release builds:

```dockerfile
ARG BUILD_PROFILE=release

# Conditionally use --release flag in cargo commands based on BUILD_PROFILE
```

### Completion Summary

Task 4 is complete. The implementation provides:

1. **Unified Configuration**: Single `docker-compose.template.yaml` used in both
   local and production environments
2. **Safe Defaults**: Local/debug mode is default; production requires explicit
   `--prod` flag
3. **Debug Builds**: Local mode uses debug builds for faster iteration
4. **Production Parity**: Same docker-compose structure tested locally and
   deployed to production
5. **Simple Interface**: `nix run .#prepDockerCompose` for local,
   `nix run .#prepDockerCompose -- --prod` for production

The reporter container now runs alongside the main bot, sharing the SQLite
database via WAL mode for concurrent access. Both containers use the same Docker
image with different commands.

---

## Task 5. Deployment Integration

Update CI/CD workflow to deploy and health check reporter.

### Rationale

Deployment must verify reporter starts successfully and doesn't have errors in
logs.

### Subtasks

- [x] Verify `Dockerfile` builds reporter binary (update if needed)
- [x] Update `.github/workflows/deploy.yaml` health checks for reporter
- [x] Add reporter log capture in deployment script
- [x] Add error detection in reporter logs
- [ ] Test full deployment workflow

### Deployment Health Checks

Add to `.github/workflows/deploy.yaml` after existing health checks:

```bash
# Check if reporter container is running
if ! docker ps --format "{{.Names}}" | grep -q "^reporter$"; then
  echo "$(date '+%Y-%m-%d %H:%M:%S') - Reporter container not found in docker ps" | tee -a "${DEPLOY_LOG}" >&2
  docker compose logs reporter 2>&1 | tee -a "${DEPLOY_LOG}"
  exit 1
fi

echo "$(date '+%Y-%m-%d %H:%M:%S') - Reporter confirmed running" | tee -a "${DEPLOY_LOG}"

# Check for errors in reporter logs
if docker compose logs reporter 2>&1 | grep -q "error\|panic\|failed"; then
  echo "$(date '+%Y-%m-%d %H:%M:%S') - Reporter errors detected in logs" | tee -a "${DEPLOY_LOG}" >&2
  docker compose logs --tail 20 reporter 2>&1 | tee -a "${DEPLOY_LOG}"
  exit 1
fi
```

### Completion Summary

Task 5 is complete. The implementation provides:

1. **Dockerfile Verification**: Confirmed reporter binary is built in both debug
   and release modes
2. **Container Health Check**: Added check to verify reporter container is
   running via `docker ps`
3. **Log Capture**: Reporter logs are captured alongside schwarbot logs to the
   Docker log file
4. **Error Detection**: Added grep-based error detection for reporter logs
   (checking for "error", "panic", "failed")
5. **Deployment Failure Handling**: If reporter container is not running or has
   errors, deployment fails with diagnostic output

The deployment workflow now:

- Verifies all three containers are running (schwarbot, grafana, reporter)
- Captures logs from both schwarbot and reporter
- Checks for errors in both service logs
- Fails fast if reporter is not healthy, preventing bad deployments

The final subtask (Test full deployment workflow) should be completed when the
code is actually deployed to production.

---

## Task 6. Testing and Validation

Comprehensive testing to ensure FIFO correctness and service reliability.

### Rationale

P&L calculations must be absolutely correct for financial applications. Testing
validates FIFO algorithm handles all scenarios correctly.

### Subtasks

- [ ] Unit tests for FIFO logic in `src/pnl/fifo.rs`
- [ ] Integration tests with SQLite in `tests/pnl_integration.rs`
- [ ] Manual validation with requirements doc 7-step example
- [ ] End-to-end test with realistic trade sequence
- [ ] Verify metrics_pnl data is queryable in Grafana (manual check)

### Unit Tests

Test file: `src/pnl/fifo.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_simple_buy_sell() {
        let mut fifo = FifoInventory::new("AAPL".to_string());

        // Buy 100 @ 10.00
        let result = fifo.process_trade(dec!(100), dec!(10.00), Direction::Long).unwrap();
        assert_eq!(result.realized_pnl, None);
        assert_eq!(result.net_position_after, dec!(100));

        // Sell 100 @ 11.00
        let result = fifo.process_trade(dec!(100), dec!(11.00), Direction::Short).unwrap();
        assert_eq!(result.realized_pnl, Some(dec!(100.00))); // (11-10)*100
        assert_eq!(result.cumulative_pnl, dec!(100.00));
        assert_eq!(result.net_position_after, dec!(0));
    }

    #[test]
    fn test_multiple_lots_fifo() {
        let mut fifo = FifoInventory::new("AAPL".to_string());

        // Buy 100 @ 10.00
        fifo.process_trade(dec!(100), dec!(10.00), Direction::Long).unwrap();
        // Buy 50 @ 12.00
        fifo.process_trade(dec!(50), dec!(12.00), Direction::Long).unwrap();

        // Sell 80 @ 11.00 (should consume from first lot only)
        let result = fifo.process_trade(dec!(80), dec!(11.00), Direction::Short).unwrap();
        assert_eq!(result.realized_pnl, Some(dec!(80.00))); // (11-10)*80
        assert_eq!(result.net_position_after, dec!(70)); // 20 + 50 remaining
    }

    #[test]
    fn test_position_reversal() {
        let mut fifo = FifoInventory::new("AAPL".to_string());

        // Buy 100 @ 10.00
        fifo.process_trade(dec!(100), dec!(10.00), Direction::Long).unwrap();

        // Sell 150 @ 11.00 (closes 100 long, opens 50 short)
        let result = fifo.process_trade(dec!(150), dec!(11.00), Direction::Short).unwrap();
        assert_eq!(result.realized_pnl, Some(dec!(100.00))); // (11-10)*100
        assert_eq!(result.net_position_after, dec!(-50)); // 50 short
    }

    #[test]
    fn test_requirements_doc_example() {
        // Implement exact 7-step example from requirements doc
        // Step 1: BUY 100 @ 10.00
        // Step 2: BUY 50 @ 12.00
        // Step 3: SELL 80 @ 11.00 -> P&L = +80.00
        // Step 4: SELL 60 @ 9.50 -> P&L = -110.00 (cum: -30.00)
        // Step 5: BUY 30 @ 12.20
        // Step 6: SELL 70 @ 12.00 -> P&L = -6.00 (cum: -36.00)
        // Step 7: BUY 20 @ 11.50 -> P&L = +10.00 (cum: -26.00)

        // Final: 10 short @ 12.00, cumulative P&L = -26.00
    }
}
```

### Integration Tests

Test file: `tests/pnl_integration.rs`

```rust
#[tokio::test]
async fn test_reporter_processes_trades_end_to_end() {
    let pool = create_in_memory_pool().await;
    run_migrations(&pool).await;

    // Insert test trades
    insert_onchain_trade(&pool, "AAPL", 10.0, 100.0, "BUY", "2025-01-01 10:00:00").await;
    insert_onchain_trade(&pool, "AAPL", 10.0, 110.0, "SELL", "2025-01-01 10:01:00").await;

    // Run reporter
    let reporter = Reporter::new(pool.clone(), Duration::from_secs(30));
    let count = reporter.process_iteration().await.unwrap();
    assert_eq!(count, 2);

    // Verify metrics_pnl
    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 2);

    // First trade: position increase, no P&L
    assert_eq!(metrics[0].realized_pnl, None);
    assert_eq!(metrics[0].net_position_after, 10.0);

    // Second trade: position decrease, realize P&L
    assert_eq!(metrics[1].realized_pnl, Some(100.0)); // (110-100)*10
    assert_eq!(metrics[1].cumulative_pnl, 100.0);
    assert_eq!(metrics[1].net_position_after, 0.0);
}
```

---

## Task 7. Documentation

Document P&L reporter for users and future maintainers.

### Rationale

Clear documentation enables operators to use the reporter and developers to
understand/extend it.

### Subtasks

- [ ] Add "P&L Reporter" section to README.md
- [ ] Document FIFO algorithm in code comments
- [ ] Update CLAUDE.md with P&L reporter information
- [ ] Add inline documentation for complex logic

### README.md Updates

Add new section:

````markdown
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
# View recent P&L metrics
nix run .#viewPnlMetrics

# View P&L summary by symbol
nix run .#viewPnlSummary

# Run reporter
nix run .#runReporter
# or
cargo run --bin reporter
```
````

### Metrics Table Schema

Every trade gets a row in `metrics_pnl`:

- **realized_pnl**: NULL for position increases, value for position decreases
- **cumulative_pnl**: Running total of realized P&L for this symbol
- **net_position_after**: Current position after trade (positive=long,
  negative=short)

### Example

```
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

````
### CLAUDE.md Updates

Add to Architecture section:

```markdown
### P&L Reporter

**Reporter Binary (`src/bin/reporter.rs`)**

- Processes trades to calculate FIFO P&L
- Runs independently in Docker container
- Shares SQLite database via mounted volume

**Core FIFO Logic (`src/pnl/`)**

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
````

---

## Summary

This plan implements FIFO P&L calculation with:

1. **Single Table**: `metrics_pnl` serves as both metrics storage and checkpoint
2. **Simple Checkpoint**: Use MAX(timestamp) instead of separate table
3. **In-Memory FIFO**: Fast state rebuild by replaying all trades on startup
4. **Rust Decimal**: Financial precision, explicit error handling
5. **Flake.nix Scripts**: Development tools follow project patterns
6. **Docker Compose**: Separate service shares database via volume
7. **Data Focus**: Get metrics into database; Grafana dashboards later

The implementation handles all FIFO scenarios including position reversals,
fractional shares, and the 7-step example from requirements.
