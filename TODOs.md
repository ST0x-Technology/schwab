# Implementation Plan: Trade Accumulation and Batching for Fractional Shares

Based on the design decision that Schwab API doesn't support fractional shares but our onchain tokenized stocks do, this plan implements a focused trade accumulation and batching system integrated directly into the existing trade processing flow.

## Task 1. Design Minimal Database Schema for Trade Accumulation

Create minimal database schema to track running net positions per symbol:

**Database Schema Design:**
- Create `trade_accumulator` table to track running net positions per symbol
- Add `batch_executions` table to record when accumulated positions are executed as batches
- Link individual trades to their contributing batch execution

**Implementation Tasks:**
- [ ] Create SQLx migration for `trade_accumulator` table with columns:
  - `symbol: TEXT PRIMARY KEY` (e.g., "AAPL") 
  - `net_position: REAL` (running sum, can be positive/negative)
  - `last_updated: TIMESTAMP`
- [ ] Create SQLx migration for `batch_executions` table with columns:
  - `id: INTEGER PRIMARY KEY`
  - `symbol: TEXT`
  - `executed_shares: INTEGER` (whole shares executed on Schwab)
  - `direction: TEXT` (BUY/SELL)
  - `schwab_order_id: TEXT`
  - `executed_at: TIMESTAMP`
- [ ] Add `batch_execution_id: INTEGER` foreign key to existing `trades` table
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
```

## Task 2. Add Position Accumulation Logic to Trade Processing

Integrate position accumulation directly into the existing trade processing flow in `@src/lib.rs`:

**Core Logic:**
- Before executing Schwab trade, update accumulated position for the symbol
- Calculate position delta based on onchain trade direction and quantity
- Atomically update `trade_accumulator` table with new net position
- Determine if accumulated position is ready for batch execution (>= 1 whole share)

**Implementation Tasks:**
- [ ] Add position accumulation functions to existing trade processing:
  - `update_accumulated_position(pool: &SqlitePool, symbol: &str, delta: f64) -> Result<f64, sqlx::Error>`
  - `get_executable_batch(pool: &SqlitePool, symbol: &str) -> Result<Option<(i32, String)>, sqlx::Error>`
  - Extract symbol from `ArbTrade` (remove "s1" suffix to get base symbol like "AAPL")
  - Calculate position delta based on onchain trade direction and quantity
- [ ] Modify `process_trade` function in `@src/lib.rs` to call accumulation before Schwab execution
- [ ] Add database transaction handling to ensure atomic position updates
- [ ] Add comprehensive error handling for database operations
- [ ] Add unit tests for position accumulation functions with in-memory SQLite
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy` 
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update TODOs.md with completion status

**Key Design Patterns:**
- Use existing SQLite connection patterns from `@src/arb.rs`
- Follow existing error handling patterns
- Implement atomic database operations using transactions

## Task 3. Implement Batch Execution Check Before Schwab Trade

Add batch execution logic that checks accumulated positions before executing individual Schwab trades:

**Batch Execution Flow:**
- After updating accumulated position, check if >= 1 whole share is ready
- If yes, execute batch trade for whole shares and update accumulated position
- If no, skip Schwab execution and just record the accumulated trade
- Record batch execution details for audit

**Implementation Tasks:**
- [ ] Add batch execution functions:
  - `execute_batch_if_ready(pool: &SqlitePool, schwab_env: &SchwabEnv, symbol: &str) -> Result<Option<BatchResult>, BatchError>`
  - `record_batch_execution(pool: &SqlitePool, symbol: &str, shares: i32, direction: &str, order_id: &str) -> Result<i64, sqlx::Error>`
- [ ] Implement `BatchResult` struct to represent completed batch operations
- [ ] Modify `process_trade` function to call batch execution check after position accumulation
- [ ] Handle batch execution results: update accumulated positions and link trades to batches
- [ ] Add comprehensive error handling for Schwab API failures, database errors
- [ ] Handle edge cases: order rejections, insufficient buying power
- [ ] Add integration tests using httpmock for Schwab API and in-memory database
- [ ] Add unit tests for batch logic with mocked dependencies  
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt` 
- [ ] Update TODOs.md with completion status

**Batch Execution Example:**
```rust
// Example flow:
// 1. Trade 1: 0.3 AAPL → accumulate, no execution (total: 0.3)
// 2. Trade 2: 0.5 AAPL → accumulate, no execution (total: 0.8) 
// 3. Trade 3: 0.4 AAPL → accumulate, execute 1 AAPL batch (remaining: 0.2)
```

## Implementation Notes

### Architecture Overview

**Integration Approach:**
- No separate services - accumulation logic integrated directly into existing `process_trade` function
- Event-driven batching - check for executable positions after each trade accumulation
- Database-centric approach - all position state persisted in SQLite

### Key Design Decisions

1. **Minimal Integration**: Add accumulation logic directly to existing trade processing flow
2. **Event-Driven Batching**: Check for executable positions after each trade instead of background processing  
3. **Database-Centric Approach**: All position state persisted in SQLite for crash recovery
4. **Preserve Existing Architecture**: Maintain current async processing, only add accumulation step
5. **Backward Compatibility**: All existing functionality (CLI, auth) remains unchanged

### Risk Mitigation

- **Maximum Fractional Exposure**: Limited to <1 share per symbol (worst case: N symbols × 0.99 shares each)
- **Position Persistence**: All positions survive bot restarts via database persistence
- **Audit Trail**: Complete linkage from individual trades to batch executions
- **Error Recovery**: Failed batch executions don't lose position data

### Testing Strategy

- **Unit Tests**: Mock database for position accumulation and batch logic testing
- **Integration Tests**: Full workflow testing with httpmock for Schwab API  
- **Realistic Scenarios**: Test scenarios matching README.md examples (0.3 + 0.5 + 0.4 = 1.2 AAPL)