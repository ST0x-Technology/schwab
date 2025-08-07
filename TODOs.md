# Implementation Plan: Trade Accumulation and Batching for Fractional Shares

Based on the design decision that Schwab API doesn't support fractional shares but our onchain tokenized stocks do, this plan implements a proper separation between onchain trades and Schwab executions with a many-to-one relationship.

## Task 1. Design Proper Database Schema Separating Onchain Trades from Schwab Executions

**Problem with Current Approach:**
The existing `trades` table incorrectly assumes a one-to-one relationship between onchain and Schwab trades, mixing onchain and Schwab data in the same row. With fractional share accumulation, we need a many-to-one relationship: multiple onchain fractional trades accumulate to trigger one whole-share Schwab execution.

**Corrected Database Schema Design:**

1. **Onchain Trades Table**: Records each blockchain event separately (fractional amounts allowed)
2. **Schwab Executions Table**: Records each Schwab API execution (whole shares only) 
3. **Position Accumulator Table**: Tracks running net positions per symbol
4. **Trade Executions Linkage Table**: Links multiple onchain trades to their contributing Schwab execution

**Implementation Tasks:**
- [ ] Create comprehensive database migration to replace existing schema:
  - Create new `onchain_trades` table for blockchain events:
    - `id: INTEGER PRIMARY KEY`
    - `tx_hash: TEXT NOT NULL`
    - `log_index: INTEGER NOT NULL`
    - `symbol: TEXT NOT NULL` (base symbol like "AAPL")
    - `amount: REAL NOT NULL` (fractional shares, positive=buy, negative=sell)
    - `price_usdc: REAL NOT NULL`
    - `status: TEXT CHECK (status IN ('PENDING', 'ACCUMULATED', 'EXECUTED'))`
    - `created_at: TIMESTAMP DEFAULT CURRENT_TIMESTAMP`
    - `UNIQUE (tx_hash, log_index)`
  - Create `schwab_executions` table for Schwab API calls:
    - `id: INTEGER PRIMARY KEY`
    - `symbol: TEXT NOT NULL`
    - `shares: INTEGER NOT NULL` (whole shares only)
    - `direction: TEXT CHECK (direction IN ('BUY', 'SELL'))`
    - `order_id: TEXT`
    - `price_cents: INTEGER`
    - `status: TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED'))`
    - `executed_at: TIMESTAMP DEFAULT CURRENT_TIMESTAMP`
  - Create `position_accumulator` table:
    - `symbol: TEXT PRIMARY KEY`
    - `net_position: REAL NOT NULL DEFAULT 0.0`
    - `last_updated: TIMESTAMP DEFAULT CURRENT_TIMESTAMP`
  - Create `trade_executions` linkage table:
    - `onchain_trade_id: INTEGER REFERENCES onchain_trades(id)`
    - `schwab_execution_id: INTEGER REFERENCES schwab_executions(id)`
    - `PRIMARY KEY (onchain_trade_id, schwab_execution_id)`
  - Migrate existing data from old `trades` table to new schema
  - Create proper indexes for efficient lookups
- [ ] Update Rust data structures to match new schema
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`

```sql
-- New Schema Design
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,
  price_usdc REAL NOT NULL,
  status TEXT CHECK (status IN ('PENDING', 'ACCUMULATED', 'EXECUTED')),
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL,
  direction TEXT CHECK (direction IN ('BUY', 'SELL')),
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')),
  executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE position_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE trade_executions (
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  PRIMARY KEY (onchain_trade_id, schwab_execution_id)
);
```

## Task 2. Implement Onchain Trade Processing with Position Accumulation

Replace the current mixed trade processing with proper separation:

**Core Logic:**
- Parse onchain events into `onchain_trades` records (fractional amounts preserved)
- Accumulate positions in `position_accumulator` table
- Trigger Schwab execution when accumulated position >= 1.0 shares
- Link contributing onchain trades to Schwab executions

**Implementation Tasks:**
- [ ] Create `OnchainTrade` struct for blockchain events
- [ ] Create `SchwabExecution` struct for Schwab API calls
- [ ] Implement position accumulation functions:
  - `save_onchain_trade() -> Result<OnchainTradeId>`
  - `update_position_accumulator() -> Result<f64>`
  - `check_executable_position() -> Result<Option<ExecutablePosition>>`
- [ ] Update `process_trade` function to use new schema
- [ ] Ensure proper deduplication using `(tx_hash, log_index)` uniqueness
- [ ] Add comprehensive error handling
- [ ] Add unit tests with in-memory SQLite
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`

**New Processing Flow:**
```rust
// 1. Parse onchain event -> OnchainTrade
let onchain_trade = OnchainTrade::from_blockchain_event(event)?;

// 2. Save with deduplication check
let trade_id = onchain_trade.save_to_db(&pool).await?;
if trade_id.is_none() {
    info!("Trade already processed, skipping");
    return Ok(());
}

// 3. Update position accumulator
let symbol = onchain_trade.symbol;
let new_position = update_position_accumulator(&pool, &symbol, onchain_trade.amount).await?;

// 4. Check if executable position available
if let Some(executable) = check_executable_position(&pool, &symbol).await? {
    // Trigger Schwab execution (Task 3)
    execute_schwab_trade(&pool, executable).await?;
}
```

## Task 3. Implement Schwab Execution with Trade Linkage Tracking

Implement Schwab API execution that properly links contributing onchain trades:

**Schwab Execution Flow:**
- When accumulated position >= 1.0 shares, execute whole shares on Schwab
- Create `schwab_executions` record with execution details
- Link all contributing `onchain_trades` via `trade_executions` table
- Update contributing onchain trades status to "EXECUTED"
- Update position accumulator to subtract executed shares (leaving fractional remainder)

**Implementation Tasks:**
- [ ] Implement Schwab execution functions:
  - `execute_schwab_trade(pool: &SqlitePool, executable: ExecutablePosition) -> Result<SchwabExecutionId>`
  - `create_schwab_execution_record() -> Result<SchwabExecutionId>`
  - `link_contributing_trades() -> Result<Vec<OnchainTradeId>>`
  - `update_position_after_execution() -> Result<f64>` (subtract executed shares)
- [ ] Create `ExecutablePosition` struct to represent ready-to-execute positions
- [ ] Implement proper transaction handling for atomic execution
- [ ] Handle Schwab API results:
  - Success: Update execution status to "COMPLETED", link trades, update positions
  - Failure: Mark execution as "FAILED", keep trades as "ACCUMULATED" for retry
- [ ] Add comprehensive error handling for Schwab API failures
- [ ] Handle edge cases: order rejections, insufficient buying power, partial fills
- [ ] Add integration tests using httpmock for Schwab API
- [ ] Add unit tests for execution logic with mocked dependencies
- [ ] Ensure tests pass: `cargo test`
- [ ] Ensure clippy passes: `cargo clippy`
- [ ] Ensure fmt passes: `cargo fmt`
- [ ] Update @TODOs.md with completion status

**Schwab Execution Example with Proper Linkage:**
```rust
// Example flow with new schema:
// OnchainTrade 1: +0.3 AAPL → save to onchain_trades (ID: 101), accumulate, position: 0.3
// OnchainTrade 2: +0.5 AAPL → save to onchain_trades (ID: 102), accumulate, position: 0.8  
// OnchainTrade 3: +0.4 AAPL → save to onchain_trades (ID: 103), accumulate, position: 1.2
//          → Position >= 1.0, trigger Schwab execution
//          → Create schwab_executions record (ID: 501): 1 AAPL BUY
//          → Link: trade_executions(101,501), trade_executions(102,501), trade_executions(103,501)
//          → Update onchain_trades 101,102,103 status to "EXECUTED"
//          → Update position_accumulator: 1.2 - 1.0 = 0.2 AAPL remaining
```

## Implementation Notes

### Architecture Overview

**Corrected Approach with Proper Schema Separation:**
- **Onchain Trades**: Each blockchain event creates one `onchain_trades` record with fractional amounts
- **Schwab Executions**: Each Schwab API call creates one `schwab_executions` record with whole shares
- **Position Accumulation**: Running totals per symbol in `position_accumulator` table
- **Trade Linkage**: Many-to-one relationship via `trade_executions` linkage table
- **Event-Driven Processing**: Check for executable positions after each onchain trade accumulation
- **Database-Centric Approach**: All state persisted in SQLite with complete audit trail

### Key Design Decisions

1. **Proper Schema Separation**: Separate tables for onchain trades vs Schwab executions (many-to-one relationship)
2. **Fractional Accumulation**: Preserve exact fractional amounts from onchain trades, execute whole shares on Schwab
3. **Deduplication by Blockchain Event**: Use `(tx_hash, log_index)` uniqueness for onchain trades
4. **Complete Audit Trail**: Every onchain trade linked to its contributing Schwab execution
5. **Atomic Transactions**: Ensure consistency between position updates, trade status, and execution linkage
6. **Preserve Async Architecture**: Maintain current async processing, replace mixed trade model with proper separation

### Benefits of New Approach

- **Accurate Fractional Tracking**: No loss of precision from onchain fractional amounts
- **Clear Separation of Concerns**: Onchain vs Schwab data properly separated
- **Scalable Relationships**: Handles complex many-to-one trade relationships
- **Better Audit Trail**: Full traceability from blockchain events to Schwab executions
- **Proper Error Handling**: Failed Schwab executions don't lose onchain trade data
- **Compliance Ready**: Clear audit trail for regulatory requirements

### Risk Mitigation

- **No Double-Processing**: Blockchain event deduplication prevents same trade from being accumulated multiple times
- **Complete Traceability**: Every onchain trade linked to contributing Schwab execution
- **Maximum Fractional Exposure**: Limited to <1 share per symbol (worst case: N symbols × 0.99 shares each)
- **Position Persistence**: All positions survive bot restarts via database persistence
- **Error Recovery**: Failed Schwab executions preserve onchain trade records and allow retry
- **Data Integrity**: Foreign key constraints ensure referential integrity between tables

### Testing Strategy

- **Schema Migration Testing**: Verify data migration from old to new schema
- **Deduplication Testing**: Verify onchain trades are only processed once per blockchain event
- **Accumulation Logic Testing**: Test fractional position accumulation with various scenarios
- **Execution Linkage Testing**: Ensure proper many-to-one linking between onchain trades and Schwab executions
- **Unit Tests**: Mock database for position accumulation and execution logic
- **Integration Tests**: Full workflow testing with httpmock for Schwab API
- **Edge Case Scenarios**: Test scenarios with failed executions, partial fills, and retry logic
