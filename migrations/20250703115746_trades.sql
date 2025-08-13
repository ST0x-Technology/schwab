
-- Onchain trades are immutable blockchain facts
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,  -- Can be fractional (e.g., 1.1 shares)
  price_usdc REAL NOT NULL,
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL, -- Whole shares only
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL DEFAULT 'PENDING',
  executed_at TIMESTAMP
);

-- Unified trade accumulator - ONE table that tracks everything
CREATE TABLE trade_accumulators (
  symbol TEXT PRIMARY KEY NOT NULL,
  net_position REAL NOT NULL DEFAULT 0.0,  -- Running position for threshold checking
  accumulated_long REAL NOT NULL DEFAULT 0.0,  -- Fractional shares accumulated for buying
  accumulated_short REAL NOT NULL DEFAULT 0.0,  -- Fractional shares accumulated for selling
  pending_execution_id INTEGER REFERENCES schwab_executions(id),  -- Current pending execution if any
  last_updated TIMESTAMP DEFAULT CURRENT_TIMESTAMP NOT NULL
);



-- Indexes for new tables
CREATE INDEX idx_onchain_trades_symbol ON onchain_trades(symbol);
CREATE INDEX idx_schwab_executions_symbol ON schwab_executions(symbol);
CREATE INDEX idx_schwab_executions_status ON schwab_executions(status);

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
