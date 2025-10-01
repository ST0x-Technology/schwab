# Plan: Add Pyth Price Data Collection to onchain_trades Table

## Overview

Add Pyth Network price data collection to the arbitrage bot to store both off-chain (Benchmarks API) and on-chain (EVM oracle) prices alongside trade records. This will enable price comparison and monitoring of oracle accuracy.

## Design Decisions

### Why Store Pyth Prices

- **Price Verification**: Compare actual trade prices against Pyth oracle prices to detect discrepancies
- **Oracle Monitoring**: Track differences between off-chain benchmark and on-chain oracle prices  
- **Audit Trail**: Maintain historical record of prices at trade execution time
- **Performance Analysis**: Analyze trade profitability relative to oracle prices

### Architecture Approach

- **Non-blocking**: Pyth price fetching should not block trade processing
- **Graceful Degradation**: Trades continue if Pyth data is unavailable
- **Efficient Caching**: Cache price feed IDs to minimize API calls
- **Reuse Infrastructure**: Leverage existing `alloy` and `reqwest` dependencies

## Implementation Plan

### Section 1: Database Schema Update

- [ ] Create new migration using `sqlx migrate add add_pyth_prices`
- [ ] Add columns to `onchain_trades` table:
  - [ ] `pyth_price_offchain` (REAL, nullable) - Benchmark price from API
  - [ ] `pyth_price_onchain` (REAL, nullable) - Oracle price from contract
  - [ ] `pyth_confidence_offchain` (REAL, nullable) - Benchmark confidence interval
  - [ ] `pyth_confidence_onchain` (REAL, nullable) - Oracle confidence interval
  - [ ] `pyth_price_timestamp` (TIMESTAMP, nullable) - When prices were fetched
- [ ] Run migration with `sqlx migrate run`
- [ ] **Checkpoint**: Verify database schema updated correctly

### Section 2: Pyth Module Structure

- [ ] Create `src/pyth/mod.rs` module structure
- [ ] Define `PythPrice` struct with price and confidence fields
- [ ] Define `PythPriceData` struct containing both off-chain and on-chain prices
- [ ] Create error types for Pyth-specific failures
- [ ] Add module to `src/lib.rs`
- [ ] **Checkpoint**: Ensure module compiles without warnings

### Section 3: Benchmarks API Client

- [ ] Create `src/pyth/benchmarks.rs`
- [ ] Implement HTTP client using existing `reqwest` dependency
- [ ] Add rate limiting (30 requests per 10 seconds)
- [ ] Implement `/v1/updates/price/{timestamp}` endpoint for historical prices
- [ ] Implement `/v1/price_feeds/` endpoint for feed discovery
- [ ] Parse JSON responses into `PythPrice` structs
- [ ] Add comprehensive error handling
- [ ] Write unit tests with mocked HTTP responses
- [ ] **Checkpoint**: Test API client with real Benchmarks API

### Section 4: On-chain Oracle Reader

- [ ] Add Pyth oracle ABI to `src/bindings.rs`
  - [ ] Contract address: `0x4305FB66699C3B2702D4d05CF36551390A4c69C6`
  - [ ] Include `getPrice(bytes32)` method
  - [ ] Include `getPriceUnsafe(bytes32)` method
- [ ] Create `src/pyth/oracle.rs`
- [ ] Implement oracle reader using existing `alloy` provider
- [ ] Parse contract responses into `PythPrice` struct
- [ ] Add error handling for contract call failures
- [ ] Write unit tests with mock provider
- [ ] **Checkpoint**: Test oracle reader on actual chain

### Section 5: Price Feed Mapping

- [ ] Create `src/pyth/feeds.rs`
- [ ] Map common equity symbols to Pyth feed IDs:
  - [ ] AAPL → Pyth feed ID
  - [ ] GOOGL → Pyth feed ID  
  - [ ] MSFT → Pyth feed ID
  - [ ] TSLA → Pyth feed ID
  - [ ] AMZN → Pyth feed ID
- [ ] Implement lazy static HashMap for lookups
- [ ] Add function to discover feed IDs via API
- [ ] Cache discovered feed IDs
- [ ] **Checkpoint**: Verify feed ID mappings are correct

### Section 6: Update OnchainTrade Struct

- [ ] Modify `src/onchain/trade.rs`
- [ ] Add optional Pyth fields to `OnchainTrade`:
  - [ ] `pyth_price_offchain: Option<f64>`
  - [ ] `pyth_price_onchain: Option<f64>`
  - [ ] `pyth_confidence_offchain: Option<f64>`
  - [ ] `pyth_confidence_onchain: Option<f64>`
  - [ ] `pyth_price_timestamp: Option<DateTime<Utc>>`
- [ ] Update `save_within_transaction` method
- [ ] Modify SQL INSERT to include Pyth columns
- [ ] Update any existing tests
- [ ] **Checkpoint**: Run existing tests to ensure no breakage

### Section 7: Integrate with Trade Processing

- [ ] Modify `src/conductor.rs`
- [ ] In `convert_event_to_trade`:
  - [ ] After creating `OnchainTrade`, spawn async task to fetch Pyth prices
  - [ ] Use block timestamp for historical benchmark lookup
  - [ ] Fetch on-chain price from oracle
  - [ ] Store prices in trade struct
- [ ] Ensure price fetching is non-blocking
- [ ] Add logging for Pyth data collection status
- [ ] Handle failures gracefully (continue trade processing)
- [ ] **Checkpoint**: Process test trade and verify Pyth data collected

### Section 8: Configuration

- [ ] Add environment variables to `.env.example`:
  - [ ] `PYTH_BENCHMARKS_API_URL` (default: https://benchmarks.pyth.network)
  - [ ] `PYTH_ORACLE_ADDRESS` (default: 0x4305FB66699C3B2702D4d05CF36551390A4c69C6)
  - [ ] `ENABLE_PYTH_PRICES` (default: true)
- [ ] Update `src/env.rs` to read new variables
- [ ] Add configuration struct for Pyth settings
- [ ] **Checkpoint**: Test with different configurations

### Section 9: Testing

- [ ] Write unit tests for Benchmarks API client
- [ ] Write unit tests for oracle reader
- [ ] Write unit tests for feed ID mapping
- [ ] Write integration test for full flow
- [ ] Test graceful degradation when Pyth unavailable
- [ ] Test database persistence of Pyth data
- [ ] Run all tests with `cargo test`
- [ ] **Checkpoint**: All tests passing

### Section 10: Final Integration

- [ ] Run full system test with real trades
- [ ] Verify Pyth prices stored in database
- [ ] Check performance impact (should be minimal)
- [ ] Verify trade processing continues if Pyth fails
- [ ] Review logs for any warnings or errors
- [ ] Run clippy and fix any issues
- [ ] Run `pre-commit run -a` to ensure formatting
- [ ] **Final Checkpoint**: System working with Pyth integration

## Success Criteria

- ✅ Trades enriched with Pyth price data when available
- ✅ Trade processing continues if Pyth unavailable  
- ✅ Both off-chain and on-chain prices stored
- ✅ No performance degradation
- ✅ Clear logging of Pyth status
- ✅ All tests passing
- ✅ No clippy warnings

## Risk Mitigation

- **API Rate Limits**: Implement proper rate limiting and caching
- **Network Failures**: Use timeouts and continue without Pyth data
- **Contract Changes**: Make oracle address configurable
- **Data Accuracy**: Store confidence intervals for reliability assessment

---

_Each section should be completed and verified before moving to the next. Checkpoints ensure the implementation is working correctly at each stage._