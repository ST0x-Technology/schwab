-- Original schema (keep existing Rust code working)
CREATE TABLE trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,

  onchain_input_symbol TEXT,
  -- TODO: Consider migrating to INTEGER (base units) or DECIMAL for exact precision
  -- Current REAL type may lose precision for 18-decimal tokenized stocks and financial calculations
  -- Will need to change for V5 orderbook upgrade (custom Float types)
  onchain_input_amount REAL,
  onchain_output_symbol TEXT,
  onchain_output_amount REAL,
  onchain_io_ratio REAL,
  onchain_price_per_share_cents REAL,

  schwab_ticker TEXT,
  schwab_instruction TEXT CHECK (schwab_instruction IN ('BUY', 'SELL')),
  schwab_quantity INTEGER,
  schwab_price_per_share_cents REAL,

  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')),
  schwab_order_id TEXT,
  created_at DATETIME NOT NULL,
  completed_at DATETIME,

  UNIQUE (tx_hash, log_index)
);

-- New schema for batching functionality (Phase 1A: Additive)
CREATE TABLE onchain_trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,
  symbol TEXT NOT NULL,
  amount REAL NOT NULL,
  price_usdc REAL NOT NULL,
  status TEXT CHECK (status IN ('PENDING', 'ACCUMULATED', 'EXECUTED')) NOT NULL DEFAULT 'PENDING',
  created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_executions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  symbol TEXT NOT NULL,
  shares INTEGER NOT NULL, -- Whole shares only
  direction TEXT CHECK (direction IN ('BUY', 'SELL')) NOT NULL,
  order_id TEXT,
  price_cents INTEGER,
  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')) NOT NULL DEFAULT 'PENDING',
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

-- Indexes for new tables
CREATE INDEX idx_onchain_trades_symbol ON onchain_trades(symbol);
CREATE INDEX idx_onchain_trades_status ON onchain_trades(status);
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
