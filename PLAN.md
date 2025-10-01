# Plan: Extract Exact Pyth Prices from Transaction Traces

## Overview

Extract the exact Pyth oracle price that was returned during onchain trade execution by analyzing transaction traces using `debug_traceTransaction`. This provides the precise price value that rain.interpreter received from the Pyth contract during trade execution, ensuring maximum accuracy for price verification and arbitrage monitoring.

## Design Decisions

### Why Transaction Traces

- **Exact Accuracy**: Captures the actual return value from Pyth oracle contract calls during execution, not approximations
- **No Timing Issues**: Shows precisely what price was used, regardless of when other price updates occurred
- **Handles Edge Cases**: Works even when multiple price updates occur in the same block or transaction
- **Verifiable Audit Trail**: Provides cryptographic proof of prices used in trades
- **No External Dependencies**: Doesn't rely on off-chain APIs that might not match on-chain reality

### Why Not Other Approaches

- **eth_call at Historical Block**: Pyth only stores latest price, not historical prices - won't work
- **Benchmarks API**: Off-chain approximation, may not match exact on-chain price used
- **Current Contract State**: Oracle has been updated since, historical price is gone

### Architecture Approach

- **dRPC Provider**: Free tier (210M compute units/month) supports debug/trace APIs
  - Supports both WebSocket and HTTP connections
  - Test if `debug_traceTransaction` works over WebSocket (preferred: single URL)
  - If not, use HTTP for debug calls and WebSocket for event subscriptions
- **CLI-First Development**: Build standalone CLI command to test extraction before integrating
- **Non-blocking Extraction**: Price extraction happens asynchronously after trade processing
- **Fail-Fast Errors**: If we can't get exact prices, log error and continue with NULL values
- **No Fallbacks**: Focus on getting accurate data, not approximate fallbacks
- **Minimal Dependencies**: Leverage existing `alloy` and `reqwest` infrastructure

## Implementation Plan

### Task 1: RPC Provider Setup and Testing

- [x] Test if `debug_traceTransaction` works over WebSocket:
  - [x] Connect to WebSocket endpoint
  - [x] Call `debug_traceTransaction` with test tx hash
  - [x] If successful: use WebSocket for everything (preferred)
  - [x] If fails: use HTTP for debug calls, WebSocket for subscriptions
- [x] Update `.env.example`:
  - [x] If WebSocket works: `WS_RPC_URL=wss://lb.drpc.org/base/API_KEY`
  - [x] If WebSocket doesn't work: `WS_RPC_URL=wss://lb.drpc.org/base/API_KEY` + `HTTP_RPC_URL=https://lb.drpc.org/base/API_KEY`
  - [x] Document dRPC free tier: 210M compute units/month
  - [x] Note that debug/trace methods are included
- [x] Update `src/env.rs` if HTTP endpoint needed:
  - [x] Add `http_rpc_url` field (optional, only if WebSocket doesn't work for debug)
- [x] **Checkpoint**: Verify RPC endpoint(s) accessible and support debug_traceTransaction

### Task 2: Pyth Module and Error Types

- [ ] Create `src/pyth/mod.rs` module structure
- [ ] Define error types in `src/pyth/mod.rs`:
  - [ ] `PythError::TraceFailed` - debug_traceTransaction RPC call failed
  - [ ] `PythError::NoPythCall` - No Pyth oracle call found in transaction trace
  - [ ] `PythError::DecodeError` - Failed to decode Pyth return data
  - [ ] `PythError::InvalidResponse` - Pyth response structure invalid
  - [ ] Implement `thiserror::Error` for all variants
- [ ] Define `PythPrice` struct:
  - [ ] `price: i64` - Raw price value from oracle
  - [ ] `conf: u64` - Confidence interval
  - [ ] `expo: i32` - Exponent (power of 10)
  - [ ] `publish_time: u64` - Unix timestamp of price publication
- [ ] Add module declaration to `src/lib.rs`
- [ ] **Checkpoint**: Module compiles without warnings

### Task 3: Debug Trace RPC Client

- [ ] Create `src/pyth/trace_client.rs`
- [ ] Implement `debug_traceTransaction` call:
  - [ ] Accept provider as parameter (WebSocket or HTTP depending on Task 1 results)
  - [ ] Build JSON-RPC request with `{"tracer": "callTracer"}` params
  - [ ] Use alloy's RPC client to send request
  - [ ] Parse JSON-RPC response into trace structure
  - [ ] Handle JSON-RPC error responses
- [ ] Define trace response structures:
  - [ ] `CallFrame` struct with `type`, `from`, `to`, `input`, `output`, `calls` fields
  - [ ] Use `serde` for JSON deserialization
- [ ] Add timeout handling (10 second timeout for trace calls)
- [ ] **Checkpoint**: Successfully retrieve trace for a test transaction on Base network

### Task 4: Trace Parsing

- [ ] Create `src/pyth/trace_parser.rs`
- [ ] Define Pyth contract address constant:
  - [ ] Base network: `0x4305FB66699C3B2702D4d05CF36551390A4c69C6`
- [ ] Define Pyth method selectors:
  - [ ] Calculate `getPriceNoOlderThan(bytes32,uint256)` selector
  - [ ] Calculate `getPriceUnsafe(bytes32)` selector
  - [ ] Calculate `getEmaPriceNoOlderThan(bytes32,uint256)` selector
- [ ] Implement recursive trace traversal:
  - [ ] Function to find all calls to Pyth contract address
  - [ ] Check `to` field matches Pyth contract
  - [ ] Check `input` field starts with known Pyth method selector
  - [ ] Extract `output` field containing return data
  - [ ] Recursively search nested `calls` array
- [ ] Handle multiple Pyth calls in single transaction:
  - [ ] Return Vec of all Pyth calls found
  - [ ] Include call depth for debugging
- [ ] **Checkpoint**: Parse test transaction trace and identify Pyth oracle calls

### Task 5: Pyth Response Decoder

- [ ] Create `src/pyth/decoder.rs`
- [ ] Research Pyth contract ABI for return types:
  - [ ] Pyth returns struct: `(int64 price, uint64 conf, int32 expo, uint publishTime)`
  - [ ] Document exact ABI encoding format
- [ ] Implement ABI decoder using `alloy::sol_types`:
  - [ ] Define Solidity type using `sol!` macro
  - [ ] Decode bytes into tuple
  - [ ] Handle decoding errors explicitly
- [ ] Implement conversion to `PythPrice` struct:
  - [ ] Extract individual fields from decoded tuple
  - [ ] Validate field values are reasonable
- [ ] Implement human-readable price conversion:
  - [ ] Add `to_decimal()` method: `price × 10^expo`
  - [ ] Use `rust_decimal::Decimal` for precision
  - [ ] Handle negative exponents correctly
- [ ] Write unit tests:
  - [ ] Test decoding valid Pyth responses
  - [ ] Test various exponent values (-8, -6, 0, etc.)
  - [ ] Test error handling for malformed data
- [ ] **Checkpoint**: Successfully decode Pyth response from real transaction

### Task 6: Price Extraction Implementation

- [ ] Create `src/pyth/extractor.rs`
- [ ] Implement main extraction function:
  - [ ] `async fn extract_pyth_price(tx_hash: B256, provider: &impl Provider) -> Result<PythPrice, PythError>`
  - [ ] Call trace client to get transaction trace
  - [ ] Call trace parser to find Pyth calls
  - [ ] If multiple Pyth calls found, use first one (or most recent based on depth)
  - [ ] Call decoder to parse return data
  - [ ] Return PythPrice struct
- [ ] Error handling:
  - [ ] Propagate all errors explicitly (no unwrap, no defaults)
  - [ ] Return Err if trace fails
  - [ ] Return Err if no Pyth call found
  - [ ] Return Err if decode fails
  - [ ] Add context to errors using `.map_err()`
- [ ] Add structured logging:
  - [ ] Debug: "Fetching trace for tx {tx_hash}"
  - [ ] Debug: "Found {n} Pyth calls in trace"
  - [ ] Info: "Extracted Pyth price: {price} (expo: {expo}, conf: {conf})"
  - [ ] Warn: "No Pyth call found in transaction {tx_hash}"
  - [ ] Error: "Failed to extract Pyth price from {tx_hash}: {error}"
- [ ] **Checkpoint**: Extract price from real Base transaction with Pyth oracle call

### Task 7: CLI Command for Testing

- [ ] Create new CLI command in `src/bin/cli.rs`:
  - [ ] Add `get-pyth-price` subcommand
  - [ ] Accept transaction hash as argument
  - [ ] Load environment config to get RPC URL
  - [ ] Create provider (WebSocket or HTTP based on Task 1)
  - [ ] Call `extract_pyth_price` function
  - [ ] Display results in human-readable format:
    - [ ] Raw price, confidence, exponent, publish time
    - [ ] Converted decimal price
    - [ ] Any errors encountered
- [ ] Test CLI command:
  - [ ] Run with known Base transaction containing Pyth call
  - [ ] Verify correct price extraction
  - [ ] Test with transaction without Pyth call (should error clearly)
  - [ ] Test with invalid transaction hash (should error clearly)
- [ ] **Checkpoint**: CLI command successfully extracts and displays Pyth prices

### Task 8: Database Schema Update

- [ ] Create migration: `sqlx migrate add add_pyth_prices_from_traces`
- [ ] Add columns to `onchain_trades` table:
  - [ ] `pyth_price` (REAL, nullable) - Decoded price value in decimal form
  - [ ] `pyth_confidence` (REAL, nullable) - Confidence interval in decimal form
  - [ ] `pyth_exponent` (INTEGER, nullable) - Exponent value for reference
  - [ ] `pyth_publish_time` (TIMESTAMP, nullable) - Oracle publish timestamp
  - [ ] `pyth_trace_depth` (INTEGER, nullable) - Call depth in trace (debugging aid)
- [ ] Add constraints:
  - [ ] `pyth_confidence >= 0` if not null
  - [ ] `pyth_trace_depth >= 0` if not null
- [ ] Run migration: `sqlx migrate run`
- [ ] Verify migration applied:
  - [ ] Check schema with `.schema onchain_trades` in sqlite3
  - [ ] Confirm all columns added correctly
- [ ] **Checkpoint**: Database schema updated successfully

### Task 9: Update OnchainTrade Struct

- [ ] Modify `src/onchain/trade.rs`
- [ ] Add Pyth fields to `OnchainTrade` struct:
  - [ ] `pyth_price: Option<f64>`
  - [ ] `pyth_confidence: Option<f64>`
  - [ ] `pyth_exponent: Option<i32>`
  - [ ] `pyth_publish_time: Option<DateTime<Utc>>`
  - [ ] `pyth_trace_depth: Option<u32>`
- [ ] Update `save_within_transaction` method:
  - [ ] Modify INSERT query to include new Pyth columns
  - [ ] Bind optional Pyth values (will be NULL if None)
  - [ ] Maintain existing transaction handling
- [ ] Update any existing tests that create `OnchainTrade`:
  - [ ] Set Pyth fields to None in test data
- [ ] Run existing tests to verify no breakage:
  - [ ] `cargo test -q onchain::trade`
- [ ] **Checkpoint**: All existing tests passing with new fields

### Task 10: Integration with Trade Processing

- [ ] Modify `src/conductor.rs`
- [ ] Update `convert_event_to_trade` or trade creation flow:
  - [ ] After creating `OnchainTrade` instance
  - [ ] Before saving to database
  - [ ] Spawn async task to extract Pyth price
  - [ ] Pass transaction hash and provider reference
- [ ] Handle extraction result:
  - [ ] If Ok, populate Pyth fields in trade struct
  - [ ] If Err, log error and leave Pyth fields as None
  - [ ] Save trade regardless of extraction success/failure
- [ ] Ensure non-blocking behavior:
  - [ ] Use `tokio::spawn` if needed for true parallelism
  - [ ] Or await but handle errors gracefully
  - [ ] Trade must be saved even if extraction fails
- [ ] Add comprehensive logging:
  - [ ] Info: Starting Pyth extraction for each trade
  - [ ] Success or error outcome for each extraction
- [ ] **Checkpoint**: Process test trade and verify Pyth data populated in database

### Task 11: Testing

- [ ] Write unit tests for trace client:
  - [ ] Mock provider responses for debug_traceTransaction
  - [ ] Test successful trace retrieval
  - [ ] Test RPC error handling
  - [ ] Test timeout handling
- [ ] Write unit tests for trace parser:
  - [ ] Create mock trace JSON with Pyth calls
  - [ ] Test finding Pyth calls at various depths
  - [ ] Test handling traces with no Pyth calls
  - [ ] Test handling multiple Pyth calls
  - [ ] Test filtering by method selector
- [ ] Write unit tests for decoder:
  - [ ] Test decoding valid Pyth ABI responses
  - [ ] Test various exponent values
  - [ ] Test error handling for invalid data
  - [ ] Test decimal conversion accuracy
- [ ] Write integration test:
  - [ ] Use real transaction hash from Base with known Pyth price
  - [ ] Call full extraction flow
  - [ ] Verify price extracted matches expected value
  - [ ] Requires debug RPC access (may need to skip in CI)
- [ ] Test error propagation:
  - [ ] Verify errors bubble up correctly
  - [ ] Verify trade processing continues on extraction failure
- [ ] Run all tests: `cargo test -q`
- [ ] **Checkpoint**: All tests passing

### Task 12: Configuration and Documentation

- [ ] Update `.env.example`:
  - [ ] Document RPC URL format: `wss://lb.drpc.org/base/API_KEY` or `https://lb.drpc.org/base/API_KEY`
  - [ ] Note that RPC must support `debug_traceTransaction`
  - [ ] Document dRPC free tier: 210M compute units/month
  - [ ] Include example with placeholder API_KEY
- [ ] Update any project documentation:
  - [ ] Document Pyth price extraction feature
  - [ ] Note that prices come from transaction traces
  - [ ] Explain that NULL prices mean extraction failed
  - [ ] Document CLI command for testing: `cargo run --bin cli get-pyth-price <TX_HASH>`
- [ ] Add code comments for complex trace parsing logic
- [ ] **Checkpoint**: Configuration documented clearly

### Task 13: Final Validation

- [ ] Run bot against Base network with real trades
- [ ] Monitor first few trades:
  - [ ] Check logs for Pyth extraction attempts
  - [ ] Verify extraction succeeds or fails gracefully
  - [ ] Check database for populated Pyth columns
  - [ ] Monitor dRPC compute unit usage
- [ ] Query database to validate data:
  - [ ] `SELECT * FROM onchain_trades WHERE pyth_price IS NOT NULL LIMIT 10`
  - [ ] Verify prices are reasonable for equity symbols
  - [ ] Check publish times align with trade times
  - [ ] Verify confidence intervals are present
- [ ] Test error scenarios:
  - [ ] Temporarily break RPC URL
  - [ ] Verify trades still process successfully
  - [ ] Verify clear error logging
  - [ ] Restore correct RPC URL
- [ ] Run static analysis:
  - [ ] `cargo clippy --all-targets --all-features -- -D warnings`
  - [ ] Fix all warnings by refactoring (no `#[allow]` attributes)
- [ ] Run formatter:
  - [ ] `cargo fmt`
  - [ ] `pre-commit run -a`
  - [ ] Fix any issues and re-run
- [ ] **Final Checkpoint**: System working end-to-end with accurate Pyth prices

## Success Criteria

- ✅ Transaction traces retrieved using `debug_traceTransaction`
- ✅ Pyth oracle calls identified in traces accurately
- ✅ Exact prices extracted and decoded from return data
- ✅ Prices stored in database alongside trade records
- ✅ Trade processing continues if price extraction fails
- ✅ No approximations or fallbacks - exact data only or NULL
- ✅ CLI command works for testing extraction independently
- ✅ All tests passing
- ✅ No clippy warnings
- ✅ Clear error logging for extraction failures

## Risk Mitigation

- **RPC Rate Limits**: dRPC free tier is 210M compute units/month - monitor usage
- **Compute Unit Costs**: debug_traceTransaction is expensive - track costs against quota
- **RPC Reliability**: If debug calls fail, trades still process (Pyth fields are NULL)
- **No Pyth Call in Trace**: Some trades might not use Pyth oracle - this is expected and OK
- **Decoding Failures**: Fail explicitly with error logs, never fake or approximate data
- **Performance**: Async extraction ensures trade processing pipeline not blocked
- **CLI-First Testing**: Validates implementation before integrating into main flow
- **Cost Scaling**: If free tier insufficient, dRPC has pay-as-you-go pricing

---

_Each task should be completed and verified at checkpoints before moving to the next. Checkpoints ensure implementation is working correctly at each stage._
