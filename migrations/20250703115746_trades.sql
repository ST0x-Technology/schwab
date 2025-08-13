-- Original schema (keep existing Rust code working)
CREATE TABLE trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,

  onchain_input_symbol TEXT NOT NULL,
  -- TODO: Consider migrating to INTEGER (base units) or DECIMAL for exact precision
  -- Current REAL type may lose precision for 18-decimal tokenized stocks and financial calculations
  -- Will need to change for V5 orderbook upgrade (custom Float types)
  onchain_input_amount REAL NOT NULL,
  onchain_output_symbol TEXT NOT NULL,
  onchain_output_amount REAL NOT NULL,
  onchain_io_ratio REAL NOT NULL,
  onchain_price_per_share_cents REAL NOT NULL,

  schwab_ticker TEXT NOT NULL,
  schwab_instruction TEXT CHECK (schwab_instruction IN ('BUY', 'SELL')) NOT NULL,
  schwab_quantity INTEGER NOT NULL,
  schwab_price_per_share_cents INTEGER,

  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL,
  schwab_order_id TEXT,
  created_at DATETIME NOT NULL,
  completed_at DATETIME,

  UNIQUE (tx_hash, log_index)
);

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

-- Simple junction table tracking which trades went into which executions
CREATE TABLE execution_trades (
  schwab_execution_id INTEGER REFERENCES schwab_executions(id),
  onchain_trade_id INTEGER REFERENCES onchain_trades(id),
  executed_amount REAL NOT NULL,  -- How much of the trade was executed
  PRIMARY KEY (schwab_execution_id, onchain_trade_id)
);


-- Indexes for new tables
CREATE INDEX idx_onchain_trades_symbol ON onchain_trades(symbol);
CREATE INDEX idx_schwab_executions_symbol ON schwab_executions(symbol);
CREATE INDEX idx_schwab_executions_status ON schwab_executions(status);
CREATE INDEX idx_execution_trades_execution_id ON execution_trades(schwab_execution_id);
CREATE INDEX idx_execution_trades_trade_id ON execution_trades(onchain_trade_id);

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
