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
  schwab_quantity REAL,
  schwab_price_per_share_cents REAL,

  status TEXT CHECK (status IN ('PENDING', 'COMPLETED', 'FAILED')),
  schwab_order_id TEXT,
  created_at DATETIME NOT NULL,
  completed_at DATETIME,

  UNIQUE (tx_hash, log_index)
);

/* NOTE: Storing underlying Schwab auth tokens is sensitive.
 * Ensure that this table is secured and access is controlled.
 * Consider encrypting the tokens if necessary.
 */
CREATE TABLE schwab_auth (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  access_token TEXT NOT NULL,
  access_token_fetched_at DATETIME NOT NULL,
  refresh_token TEXT NOT NULL,
  refresh_token_fetched_at DATETIME NOT NULL,
  last_updated DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TRIGGER schwab_auth_update_last_updated
AFTER UPDATE OF access_token, access_token_fetched_at, refresh_token, refresh_token_fetched_at ON schwab_auth
FOR EACH ROW
BEGIN
  UPDATE schwab_auth SET last_updated = CURRENT_TIMESTAMP WHERE id = NEW.id;
END;
