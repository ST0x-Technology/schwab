CREATE TABLE trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,

  onchain_input_symbol TEXT,
  onchain_input_amount REAL,
  onchain_output_symbol TEXT,
  onchain_output_amount REAL,
  onchain_io_ratio REAL,

  schwab_ticker TEXT,
  schwab_instruction TEXT CHECK (schwab_instruction IN ('BUY', 'SELL')),
  schwab_quantity REAL,
  schwab_price_cents INTEGER,

  status TEXT,
  schwab_order_id TEXT,
  created_at DATETIME NOT NULL,
  completed_at DATETIME,

  UNIQUE (tx_hash, log_index)
);

CREATE TABLE schwab_auth (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  access_token TEXT NOT NULL,
  access_token_fetched_at DATETIME NOT NULL,
  refresh_token TEXT NOT NULL,
  refresh_token_fetched_at DATETIME NOT NULL
);