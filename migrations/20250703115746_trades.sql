-- Onchain trades are immutable blockchain facts
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  tx_hash TEXT NOT NULL CHECK (length(tx_hash) = 66 AND tx_hash LIKE '0x%'),  -- Ensure valid transaction hash format
  log_index INTEGER NOT NULL CHECK (log_index >= 0),  -- Log index must be non-negative
  symbol TEXT NOT NULL CHECK (symbol != ''),  -- Valid symbol constraints
  amount REAL NOT NULL CHECK (amount > 0.0),  -- Trade amount must be positive (quantity only)
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,  -- Explicit trade direction for Schwab execution
  price_usdc REAL NOT NULL CHECK (price_usdc > 0.0),  -- Price must be positive
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  symbol TEXT NOT NULL CHECK (symbol != ''),  -- Valid symbol constraints
  shares INTEGER NOT NULL CHECK (shares > 0),  -- Must execute positive whole shares
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT CHECK (order_id IS NULL OR order_id != ''),  -- Valid order ID or NULL
  price_cents INTEGER CHECK (price_cents IS NULL OR price_cents >= 0),  -- Price must be non-negative or NULL
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL DEFAULT 'PENDING',
  executed_at TIMESTAMP,
  CHECK (
    (status = 'PENDING' AND order_id IS NULL AND executed_at IS NULL) OR
    (status = 'COMPLETED' AND order_id IS NOT NULL AND executed_at IS NOT NULL) OR
    (status = 'FAILED' AND executed_at IS NOT NULL)
  )
);

-- Unified trade accumulator - ONE table that tracks everything
CREATE TABLE trade_accumulators (
  symbol TEXT PRIMARY KEY NOT NULL,
  net_position REAL NOT NULL DEFAULT 0.0,  -- Running position for threshold checking
  accumulated_long REAL NOT NULL DEFAULT 0.0 CHECK (accumulated_long >= 0.0),  -- Fractional shares accumulated for buying
  accumulated_short REAL NOT NULL DEFAULT 0.0 CHECK (accumulated_short >= 0.0),  -- Fractional shares accumulated for selling
  pending_execution_id INTEGER REFERENCES schwab_executions(id) ON DELETE SET NULL ON UPDATE CASCADE,  -- Current pending execution if any
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL,
  CHECK (symbol != '')  -- Ensure symbol is not empty
);



-- Indexes for new tables
CREATE INDEX idx_onchain_trades_symbol ON onchain_trades(symbol);
CREATE INDEX idx_schwab_executions_symbol ON schwab_executions(symbol);
CREATE INDEX idx_schwab_executions_status ON schwab_executions(status);

-- Data integrity constraints
-- Ensure only one pending execution per symbol (prevents race conditions)
CREATE UNIQUE INDEX idx_unique_pending_execution_per_symbol
ON schwab_executions(symbol)
WHERE status = 'PENDING';

-- Ensure only one pending execution reference per symbol in accumulators
CREATE UNIQUE INDEX idx_unique_pending_execution_in_accumulator
ON trade_accumulators(pending_execution_id)
WHERE pending_execution_id IS NOT NULL;

/* NOTE: Storing underlying Schwab auth tokens is sensitive.
 * Ensure that this table is secured and access is controlled.
 * Consider encrypting the tokens if necessary.
 */
CREATE TABLE schwab_auth (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  access_token TEXT NOT NULL,
  access_token_fetched_at DATETIME NOT NULL,
  refresh_token TEXT NOT NULL,
  refresh_token_fetched_at DATETIME NOT NULL
);
