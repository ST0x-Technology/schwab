-- Add Pyth Network price data columns to onchain_trades table
-- These columns store both off-chain (Benchmarks API) and on-chain (oracle) prices
-- All columns are nullable to ensure trades continue processing if Pyth data is unavailable

ALTER TABLE onchain_trades ADD COLUMN pyth_price_offchain REAL;
ALTER TABLE onchain_trades ADD COLUMN pyth_price_onchain REAL;
ALTER TABLE onchain_trades ADD COLUMN pyth_confidence_offchain REAL;
ALTER TABLE onchain_trades ADD COLUMN pyth_confidence_onchain REAL;
ALTER TABLE onchain_trades ADD COLUMN pyth_price_timestamp TIMESTAMP;