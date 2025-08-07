-- Add trade accumulation and batching support

-- Table to track net positions per symbol for fractional share accumulation
CREATE TABLE trade_accumulator (
  symbol TEXT PRIMARY KEY,
  net_position REAL NOT NULL DEFAULT 0.0,
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Table to record batch executions when accumulated positions are executed
CREATE TABLE batch_executions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  symbol TEXT NOT NULL,
  executed_shares INTEGER NOT NULL,
  direction TEXT NOT NULL CHECK(direction IN ('BUY', 'SELL')),
  schwab_order_id TEXT,
  executed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Add foreign key to existing trades table to link trades to their batch execution
ALTER TABLE trades ADD COLUMN batch_execution_id INTEGER REFERENCES batch_executions(id);

-- Indexes for efficient position lookups
CREATE INDEX idx_trade_accumulator_symbol ON trade_accumulator(symbol);
CREATE INDEX idx_batch_executions_symbol ON batch_executions(symbol);
CREATE INDEX idx_trades_batch_execution_id ON trades(batch_execution_id);