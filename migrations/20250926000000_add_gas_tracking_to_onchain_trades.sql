-- Add gas tracking columns to onchain_trades table for Base L2 transactions
-- Store only raw values from transaction receipt to maintain data normalization
ALTER TABLE onchain_trades ADD COLUMN gas_used INTEGER;
ALTER TABLE onchain_trades ADD COLUMN effective_gas_price INTEGER;