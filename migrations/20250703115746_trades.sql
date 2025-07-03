CREATE TABLE trades (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tx_hash TEXT NOT NULL,
  log_index INTEGER NOT NULL,

  onchain_input_symbol TEXT,
  onchain_input_amount REAL,
  onchain_output_symbol TEXT,
  onchain_output_amount REAL,
  onchain_io_ratio REAL,

  schwab_input_symbol TEXT,
  schwab_input_amount REAL,
  schwab_output_symbol TEXT,
  schwab_output_amount REAL,
  schwab_io_ratio REAL,

  status TEXT,
  schwab_order_id TEXT,
  created_at TEXT,
  completed_at TEXT,

  UNIQUE (tx_hash, log_index)
);
