# Implementation Plan: Trade Accumulation and Batching for Fractional Shares

Based on the design decision that Schwab API doesn't support fractional shares but our onchain tokenized stocks do, this plan implements a focused trade accumulation and batching system integrated directly into the existing trade processing flow.

## Task 1. Design Minimal Database Schema for Trade Accumulation

Create minimal database schema to track running net positions per symbol and ensure proper trade linkage:

**Database Schema Design:**
- Create `trade_accumulator` table to track running net positions per symbol
- Add `batch_executions` table to record when accumulated positions are executed as batches
- Link individual trades to their contributing batch execution
- Track trade processing status to prevent double-accumulation

**Implementation Tasks:**
- [ ] Create single SQLx migration file using naming pattern:
  - Run: `touch "migrations/$(date -u +%Y%m%d%H%M%S)_add_trade_batching_support.sql"`
  - Add `trade_accumulator` table with columns:
    - `symbol: TEXT PRIMARY KEY` (e.g., "AAPL") 
    - `net_position: REAL` (running sum, can be positive/negative)
    - `last_updated: TIMESTAMP`
  - Add `batch_executions` table with columns:
    - `id: INTEGER PRIMARY KEY`
    - `symbol: TEXT`
    - `executed_shares: INTEGER` (whole shares executed on Schwab)
    - `direction: TEXT` (BUY/SELL)
    - `schwab_order_id: TEXT`
    - `executed_at: TIMESTAMP`
  - Add `batch_execution_id: INTEGER` foreign key to existing `trades` table
  - Extend existing `trades` table status to include "ACCUMULATED" status
- [ ] Create basic database indexes for efficient position lookups
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

```sql
-- Example schema additions
CREATE TABLE trade_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE batch_executions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  symbol TEXT NOT NULL,
  executed_shares INTEGER NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
  schwab_order_id TEXT,
  executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Add to existing trades table
ALTER TABLE trades ADD COLUMN batch_execution_id INTEGER REFERENCES batch_executions(id);

-- Extend status values to include accumulation tracking
-- Existing: PENDING, COMPLETED, FAILED
-- Add: ACCUMULATED (trade contributed to position but no Schwab execution yet)
```

## Task 2. Add Position Accumulation Logic with Proper Trade Deduplication

Integrate position accumulation directly into the existing trade processing flow, ensuring trades are only accumulated once:

**Core Logic:**
- Leverage existing `ArbTrade::try_save_to_db()` deduplication logic 
- Only accumulate positions for trades that are successfully saved (new trades, not duplicates)
- Update trade status to "ACCUMULATED" after successful position accumulation
- Calculate position delta based on onchain trade direction and quantity

**Implementation Tasks:**
- [ ] Add position accumulation functions to existing trade processing:
  - `update_accumulated_position(pool: &SqlitePool, symbol: &str, delta: f64) -> Result<f64, sqlx::Error>`
  - `get_executable_batch(pool: &SqlitePool, symbol: &str) -> Result<Option<(i32, String)>, sqlx::Error>`
  - Extract symbol from `ArbTrade` (remove "s1" suffix to get base symbol like "AAPL")
  - Calculate position delta based on onchain trade direction and quantity
- [ ] Modify `process_trade` function in `@src/lib.rs` to:
  - Keep existing `ArbTrade::try_save_to_db()` call for deduplication
  - Only call accumulation if trade save was successful (new trade)
  - Update trade status to "ACCUMULATED" after successful position update
  - Skip accumulation entirely if trade already exists (caught by deduplication)
- [ ] Add database transaction handling to ensure atomic position updates with status changes
- [ ] Add comprehensive error handling for database operations
- [ ] Add unit tests for position accumulation functions with in-memory SQLite
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy` 
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

**Trade Processing Flow:**
```rust
// Existing deduplication prevents double-processing
match trade.try_save_to_db(&pool).await {
    Ok(trade_id) => {
        // New trade - safe to accumulate position
        let new_position = update_accumulated_position(&pool, &symbol, delta).await?;
        trade.update_status(&pool, trade_id, "ACCUMULATED").await?;
        // Continue to batch check...
    }
    Err(duplicate_error) => {
        // Trade already processed - skip accumulation
        info!("Trade already processed, skipping accumulation");
        return Ok(());
    }
}
```

## Task 3. Implement Batch Execution with Trade Linkage Tracking

Add batch execution logic that properly links trades to their batch executions:

**Batch Execution Flow:**
- After successful position accumulation, check if >= 1 whole share is ready
- If yes, execute batch trade for whole shares and update accumulated position  
- Link all contributing trades to the batch execution via `batch_execution_id`
- Update trade status from "ACCUMULATED" to "COMPLETED" for batch-executed trades
- Record batch execution details for audit

**Implementation Tasks:**
- [ ] Add batch execution functions:
  - `execute_batch_if_ready(pool: &SqlitePool, schwab_env: &SchwabEnv, symbol: &str) -> Result<Option<BatchResult>, BatchError>`
  - `record_batch_execution(pool: &SqlitePool, symbol: &str, shares: i32, direction: &str, order_id: &str) -> Result<i64, sqlx::Error>`
  - `link_trades_to_batch(pool: &SqlitePool, symbol: &str, batch_id: i64) -> Result<Vec<i64>, sqlx::Error>`
- [ ] Implement `BatchResult` struct to represent completed batch operations with trade IDs
- [ ] Modify `process_trade` function to call batch execution check after position accumulation
- [ ] Handle batch execution results:
  - Update accumulated positions (subtract executed shares)
  - Link all accumulated trades for symbol to batch execution
  - Update trade status from "ACCUMULATED" to "COMPLETED" for batch participants
- [ ] Add comprehensive error handling for Schwab API failures, database errors
- [ ] Handle edge cases: order rejections, insufficient buying power
- [ ] Add integration tests using httpmock for Schwab API and in-memory database
- [ ] Add unit tests for batch logic with mocked dependencies  
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt` 
- [ ] Update TODOs.md with completion status

**Batch Execution Example with Trade Tracking:**
```rust
// Example flow with trade tracking:
// Trade 1: 0.3 AAPL → save to DB (ID: 101), accumulate (status: ACCUMULATED), total: 0.3
// Trade 2: 0.5 AAPL → save to DB (ID: 102), accumulate (status: ACCUMULATED), total: 0.8  
// Trade 3: 0.4 AAPL → save to DB (ID: 103), accumulate (status: ACCUMULATED), total: 1.2
//          → Execute 1 AAPL batch, create batch_execution (ID: 501)
//          → Link trades 101, 102, 103 to batch 501 (set batch_execution_id = 501)
//          → Update trades 101, 102, 103 status to "COMPLETED"
//          → Remaining accumulated position: 0.2 AAPL
```

## Implementation Notes

### Architecture Overview

**Integration Approach:**
- No separate services - accumulation logic integrated directly into existing `process_trade` function
- Leverage existing deduplication logic to prevent double-accumulation
- Event-driven batching - check for executable positions after each successful trade accumulation
- Database-centric approach - all position state persisted in SQLite with full audit trail

### Key Design Decisions

1. **Reuse Existing Deduplication**: Leverage `ArbTrade::try_save_to_db()` to prevent processing same trade multiple times
2. **Trade Status Tracking**: Extend existing status system to track accumulation and batch execution states
3. **Complete Audit Trail**: Every trade linked to its batch execution for full traceability  
4. **Atomic Operations**: Database transactions ensure consistency between position updates and trade status
5. **Preserve Existing Architecture**: Maintain current async processing, only add accumulation step

### Risk Mitigation

- **No Double-Processing**: Existing deduplication logic prevents same trade from being accumulated multiple times
- **Complete Traceability**: Every trade linked to batch execution for audit and compliance
- **Maximum Fractional Exposure**: Limited to <1 share per symbol (worst case: N symbols × 0.99 shares each)
- **Position Persistence**: All positions survive bot restarts via database persistence
- **Error Recovery**: Failed batch executions don't lose position data or trade linkage

### Testing Strategy

- **Deduplication Testing**: Verify trades are only accumulated once even with duplicate events
- **Batch Linkage Testing**: Ensure all contributing trades properly linked to batch executions
- **Unit Tests**: Mock database for position accumulation and batch logic testing
- **Integration Tests**: Full workflow testing with httpmock for Schwab API  
- **Realistic Scenarios**: Test scenarios with duplicate trade events and batch execution