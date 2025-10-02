# Plan: Extract Exact Pyth Prices from Transaction Traces

## Overview

Extract the exact Pyth oracle price that was returned during onchain trade
execution by analyzing transaction traces using `debug_traceTransaction`. This
provides the precise price value that rain.interpreter received from the Pyth
contract during trade execution, ensuring maximum accuracy for price
verification and arbitrage monitoring.

## Design Decisions

### Why Transaction Traces

- **Exact Accuracy**: Captures the actual return value from Pyth oracle contract
  calls during execution, not approximations
- **No Timing Issues**: Shows precisely what price was used, regardless of when
  other price updates occurred
- **Handles Edge Cases**: Works even when multiple price updates occur in the
  same block or transaction
- **Verifiable Audit Trail**: Provides cryptographic proof of prices used in
  trades
- **No External Dependencies**: Doesn't rely on off-chain APIs that might not
  match on-chain reality

### Why Not Other Approaches

- **eth_call at Historical Block**: Pyth only stores latest price, not
  historical prices - won't work
- **Benchmarks API**: Off-chain approximation, may not match exact on-chain
  price used
- **Current Contract State**: Oracle has been updated since, historical price is
  gone

### Architecture Approach

- **dRPC Provider**: Free tier (210M compute units/month) supports debug/trace
  APIs
  - ✅ WebSocket works for `debug_traceTransaction` - single URL for everything
  - No need for separate HTTP endpoint
- **Alloy Built-in Support**: Use alloy's native trace types instead of custom
  implementations
  - ✅ `alloy::providers::ext::DebugApi` trait provides
    `debug_trace_transaction` method
  - ✅ `GethTrace::CallTracer(CallFrame)` gives us typed trace structures
  - No need to build custom JSON-RPC requests or parse raw JSON
- **CLI-First Development**: Build standalone CLI command to test extraction
  before integrating
  - ✅ `get-pyth-price` command started with trace fetching
- **Single File Module**: Start with `src/pyth.rs`, split into submodules only
  if needed
- **Non-blocking Extraction**: Price extraction happens asynchronously after
  trade processing
- **Fail-Fast Errors**: If we can't get exact prices, log error and continue
  with NULL values
- **No Fallbacks**: Focus on getting accurate data, not approximate fallbacks
- **Minimal Dependencies**: Leverage existing `alloy` infrastructure

## Implementation Plan

### Task 1: RPC Provider Setup and Testing

- [x] Test if `debug_traceTransaction` works over WebSocket:
  - [x] Connect to WebSocket endpoint
  - [x] Call `debug_traceTransaction` with test tx hash
  - [x] If successful: use WebSocket for everything (preferred)
  - [x] If fails: use HTTP for debug calls, WebSocket for subscriptions
- [x] Update `.env.example`:
  - [x] If WebSocket works: `WS_RPC_URL=wss://lb.drpc.org/base/API_KEY`
  - [x] If WebSocket doesn't work: `WS_RPC_URL=wss://lb.drpc.org/base/API_KEY` +
        `HTTP_RPC_URL=https://lb.drpc.org/base/API_KEY`
  - [x] Document dRPC free tier: 210M compute units/month
  - [x] Note that debug/trace methods are included
- [x] Update `src/env.rs` if HTTP endpoint needed:
  - [x] Add `http_rpc_url` field (optional, only if WebSocket doesn't work for
        debug)
- [x] **Checkpoint**: Verify RPC endpoint(s) accessible and support
      debug_traceTransaction

### Task 2: Pyth Contract Dependency and ABI

**Note**: Pyth's GitHub repo (pyth-network/pyth-sdk-solidity) is deprecated and
removed as of August 2025. Use NPM package instead.

- [x] Initialize npm if not already present:
  - [x] Run `npm init -y` in project root
  - [x] Verify package.json created
- [x] Install Pyth SDK from NPM:
  - [x] Run `npm install @pythnetwork/pyth-sdk-solidity`
  - [x] Verify `node_modules/@pythnetwork/pyth-sdk-solidity/` directory created
- [x] Create/update `remappings.txt` in project root:
  - [x] Add line:
        `@pythnetwork/pyth-sdk-solidity/=node_modules/@pythnetwork/pyth-sdk-solidity/`
  - [x] This allows imports like
        `import "@pythnetwork/pyth-sdk-solidity/IPyth.sol";` in Solidity
- [x] Update `flake.nix` prepSolArtifacts task to build Pyth contracts:
  - [x] Add forge build command:
        `(cd node_modules/@pythnetwork/pyth-sdk-solidity/ && forge build)`
  - [x] Note: Pyth SDK ships with precompiled ABIs in `abis/` directory, no
        forge build needed
- [x] Add Pyth bindings to `src/bindings.rs`:
  - [x] Use `sol!` macro:
        `IPyth, "node_modules/@pythnetwork/pyth-sdk-solidity/abis/IPyth.json"`
  - [x] Add serde derives: `#[derive(serde::Serialize, serde::Deserialize)]`
  - [x] Follow pattern from existing IOrderBookV4 and IERC20 bindings
- [x] Research Pyth types from generated bindings:
  - [x] Available types: `IPyth::Price` (not PythStructs, use IPyth namespace
        directly)
  - [x] Method selectors available via
        `IPyth::getPriceNoOlderThanCall::SELECTOR`
  - [x] Fields: `price: i64, conf: u64, expo: i32, publishTime: U256` (note:
        publishTime is U256, not u64)
  - [x] Use `cargo expand` to view macro expansions if needed
- [x] **Checkpoint**: Bindings compile and Pyth types available

### Task 3: Pyth Module - Error Types

- [x] Create `src/pyth.rs` (single file to start, can split later if needed)
- [x] Define error types:
  - [x] `PythError::NoPythCall` - No Pyth oracle call found in transaction trace
  - [x] `PythError::DecodeError` - Failed to decode Pyth return data
  - [x] `PythError::InvalidResponse` - Pyth response structure invalid
  - [x] Implement `thiserror::Error` for all variants
- [x] Add module declaration to `src/lib.rs`
- [x] Define Pyth contract address constant:
  - [x] Base network: `0x8250f4aF4B972684F7b336503E2D6dFeDeB1487a`
- [x] **Checkpoint**: Module compiles without warnings
- [x] **Note**: Use `PythStructs::Price` from bindings directly, no custom
      struct needed

### Task 4: Trace Parsing (using Pyth bindings)

- [x] Implement recursive trace traversal function in `src/pyth.rs`:
  - [x] Accept `GethTrace` from alloy (already fetched via
        `fetch_transaction_trace`)
  - [x] Extract `CallFrame` from `GethTrace::CallTracer` variant
  - [x] Recursively search call tree for Pyth contract calls
  - [x] Check `to` field matches Pyth contract address
  - [x] Use Pyth bindings for method selectors (e.g.,
        `IPyth::getPriceNoOlderThanCall::SELECTOR`)
  - [x] Check `input` field starts with Pyth method selector from bindings
  - [x] Extract `output` field containing return data
- [x] Handle multiple Pyth calls in single transaction:
  - [x] Return Vec of all Pyth calls found with call depth
  - [x] Use first call for price extraction
- [x] Write unit tests with mock CallFrame structures
- [x] **Checkpoint**: Parse test transaction trace and identify Pyth oracle
      calls

### Task 5: Pyth Response Decoder and Price Conversion

- [x] Use Pyth bindings to decode output in `src/pyth.rs`:
  - [x] Use `IPyth` return types from bindings to decode `output` bytes
  - [x] Extract `PythStructs::Price` from decoded response
  - [x] Use `PythStructs::Price` directly (no wrapper struct needed)
- [x] Add price conversion helper:
  - [x] Implement `to_decimal(price: &PythStructs::Price) -> Decimal` function
  - [x] Formula: `price.price × 10^price.expo`
  - [x] Use `rust_decimal::Decimal` for precision
  - [x] Handle negative exponents correctly
- [x] Write unit tests:
  - [x] Test decoding valid Pyth responses using binding types
  - [x] Test various exponent values (-8, -6, 0, etc.)
  - [x] Test error handling for malformed data
  - [x] Test decimal conversion accuracy
- [x] **Checkpoint**: Successfully decode Pyth response from real transaction

### Task 6: Price Extraction Implementation

- [x] Implement main extraction function in `src/pyth.rs`:
  - [x] `pub async fn extract_pyth_price(tx_hash: B256, provider: &impl Provider) -> Result<PythStructs::Price, PythError>`
  - [x] Call `fetch_transaction_trace` (moved from cli.rs to pyth.rs)
  - [x] Call trace parser to find Pyth calls
  - [x] If multiple Pyth calls found, use first one
  - [x] Decode output bytes using Pyth bindings from `src/bindings.rs`
  - [x] Return `PythStructs::Price` directly
- [x] Error handling:
  - [x] Propagate all errors explicitly (no unwrap, no defaults)
  - [x] Return Err if no Pyth call found
  - [x] Return Err if decode fails
  - [x] Add context to errors using `.map_err()`
- [x] Add structured logging:
  - [x] Debug: "Fetching trace for tx {tx_hash}"
  - [x] Debug: "Found {n} Pyth calls in trace"
  - [x] Info: "Extracted Pyth price: {price} (expo: {expo}, conf: {conf})"
  - [x] Warn: "No Pyth call found in transaction {tx_hash}"
  - [x] Error: "Failed to extract Pyth price from {tx_hash}: {error}"
- [x] **Checkpoint**: Extract price from real Base transaction with Pyth oracle
      call

### Task 7: CLI Command Integration

- [x] Create new CLI command in `src/cli.rs`:
  - [x] Add `get-pyth-price` subcommand
  - [x] Accept transaction hash as argument
  - [x] Load environment config to get RPC URL
  - [x] Create WebSocket provider
  - [x] Call `fetch_transaction_trace` (implemented with alloy's DebugApi)
- [x] Wire up price extraction:
  - [x] Call `extract_pyth_price` function from pyth module
  - [x] Display results in human-readable format:
    - [x] Raw price, confidence, exponent, publish time
    - [x] Converted decimal price
    - [x] Any errors encountered
- [ ] Test CLI command:
  - [ ] Run with known Base transaction containing Pyth call
        (0xa207d7abf2aa69badb2d4b266b5d2ed03ec10c4f0de173b866815714b75e055f)
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
  - [ ] `pyth_trace_depth` (INTEGER, nullable) - Call depth in trace (debugging
        aid)
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
- [ ] **Checkpoint**: Process test trade and verify Pyth data populated in
      database

### Task 11: Testing

- [x] Write unit tests for trace fetching:
  - [x] Mock provider responses for debug_traceTransaction
  - [x] Test successful trace retrieval (test_fetch_transaction_trace in cli.rs)
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
  - [ ] Document RPC URL format: `wss://lb.drpc.org/base/API_KEY` or
        `https://lb.drpc.org/base/API_KEY`
  - [ ] Note that RPC must support `debug_traceTransaction`
  - [ ] Document dRPC free tier: 210M compute units/month
  - [ ] Include example with placeholder API_KEY
- [ ] Update any project documentation:
  - [ ] Document Pyth price extraction feature
  - [ ] Note that prices come from transaction traces
  - [ ] Explain that NULL prices mean extraction failed
  - [ ] Document CLI command for testing:
        `cargo run --bin cli get-pyth-price <TX_HASH>`
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

- **RPC Rate Limits**: dRPC free tier is 210M compute units/month - monitor
  usage
- **Compute Unit Costs**: debug_traceTransaction is expensive - track costs
  against quota
- **RPC Reliability**: If debug calls fail, trades still process (Pyth fields
  are NULL)
- **No Pyth Call in Trace**: Some trades might not use Pyth oracle - this is
  expected and OK
- **Decoding Failures**: Fail explicitly with error logs, never fake or
  approximate data
- **Performance**: Async extraction ensures trade processing pipeline not
  blocked
- **CLI-First Testing**: Validates implementation before integrating into main
  flow
- **Cost Scaling**: If free tier insufficient, dRPC has pay-as-you-go pricing

---

_Each task should be completed and verified at checkpoints before moving to the
next. Checkpoints ensure implementation is working correctly at each stage._
