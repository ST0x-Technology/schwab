# HyperDX Observability Integration Plan

## Overview

Integrate HyperDX observability into st0x-hedge using the working reference
implementation from
https://gist.github.com/rouzwelt/6d67cb11950c6b286f3350aa17558585 as a guide.
We'll start with an exact copy in a test binary to verify it works with HyperDX,
then gradually integrate the telemetry setup into the main codebase following
our guidelines, verifying HyperDX connection after each major step.

## Design Decisions

### Two-Phase Integration Approach

We'll use a two-phase approach to ensure we maintain a working HyperDX
connection throughout:

1. **Phase 1 - Establish Baseline**: Create a standalone test binary with the
   EXACT reference code to verify HyperDX connectivity works
2. **Phase 2 - Gradual Integration**: Incrementally integrate telemetry into the
   main codebase, checking HyperDX after each task

**Rationale**: This approach gives us a known-working baseline before making any
modifications. If something breaks during integration, we can compare against
the working reference.

### Batch Exporter Architecture

Use batch span processor (not simple exporter) as shown in the reference
implementation:

- Better performance for production workloads
- Reduced network overhead through batching
- Configurable batch size, queue size, and delay
- **Critical Detail**: Requires blocking reqwest client spawned on separate
  thread to avoid tokio runtime conflicts with `BatchSpanProcessor`

**Rationale**: The reference implementation explicitly uses batch exporter with
detailed comments explaining why. The batch processor uses a thread pool that
would conflict with tokio's main runtime if using an async client.

### Optional Telemetry

Telemetry must be completely optional:

- Bot runs normally without HyperDX configuration
- No panics or errors if HyperDX is unavailable
- Telemetry setup returns `Option<T>` or `Result<T, E>`
- Graceful degradation to console-only logging

**Rationale**: Follows our existing pattern for optional features and ensures
the bot can run in environments without observability infrastructure.

### Environment Configuration

Use these environment variables:

- `HYPERDX_API_KEY` - HyperDX API key (authorization header, NO "Bearer" prefix
  per reference code)
- `HYPERDX_SERVICE_NAME` - Service name (default: "st0x-hedge")
- `HYPERDX_ENDPOINT` - OTLP endpoint (default:
  "https://in-otel.hyperdx.io/v1/traces")

**Rationale**: Matches reference implementation pattern. API key in
authorization header without Bearer prefix is specific to HyperDX requirements.

### Instrumentation Strategy

Add instrumentation progressively by component:

1. Main event loop first (highest level)
2. Trade processing (core business logic)
3. Broker integration (order execution)
4. Conductor (orchestration)

**Rationale**: Start with high-level spans to see overall application flow, then
add detail. This allows us to verify at each level that parent-child
relationships are correct.

---

## Task 1. Create Test Binary with Reference Implementation ✅ COMPLETED

Create a new test binary containing the EXACT reference implementation to
establish a verified baseline.

- [x] Create `src/bin/test_hyperdx.rs`
- [x] Copy the exact `main.rs` code from the gist into `test_hyperdx.rs`
- [x] Keep `IS_BATCH_EXPORTER = true` (use batch exporter as reference does)
- [x] Update workspace `Cargo.toml` dependencies to add/update:
  - [x] Add `gzip` and `blocking` features to existing `reqwest` workspace
        dependency
  - [x] Add `opentelemetry = "0.30.0"` to workspace dependencies
  - [x] Add
        `opentelemetry_sdk = { version = "0.30.0", features = ["rt-tokio"] }` to
        workspace dependencies
  - [x] Add `opentelemetry-otlp = "0.30.0"` to workspace dependencies
  - [x] Add `tracing-opentelemetry = "0.31.0"` to workspace dependencies
- [x] Make sure main package references these workspace dependencies
- [x] Verify binary compiles: `cargo build --bin test_hyperdx`

**Implementation Summary:**

Created `src/bin/test_hyperdx.rs` with exact copy of reference implementation
including:

- Batch span processor with blocking reqwest client spawned on separate thread
- gRPC protocol for OTLP export
- Batch config: 512 batch size, 2048 queue size, 3s scheduled delay
- Test spans demonstrating parent-child relationships and instrumentation
  patterns

Updated `Cargo.toml`:

- Added `gzip` and `blocking` features to reqwest workspace dependency
- Added all OpenTelemetry dependencies to workspace.dependencies
- Referenced them in main package dependencies
- Binary compiles successfully (verified with `cargo check --bin test_hyperdx`)

**Why exact copy**: We need to establish a known-working baseline before making
any modifications. The reference implementation has specific details (blocking
client, thread spawn, gRPC protocol) that are critical for HyperDX to work.

---

## Task 2. Verify Test Binary Works with HyperDX ✅ COMPLETED

Run the test binary and have the user confirm traces appear in HyperDX
dashboard.

- [x] Replace hardcoded `"api-key"` string in test binary with actual API key
      from environment variable
- [x] Run test binary: `HYPERDX_API_KEY=<real-key> cargo run --bin test_hyperdx`
- [x] Wait for the 10 second sleep to complete
- [x] **USER VERIFICATION REQUIRED**: Ask user to check HyperDX dashboard and
      confirm:
  - [x] Service "my-service-name" appears
  - [x] Root span `app_start` is visible with `work_units = 2` attribute
  - [x] Child span `app_start_child` appears under `app_start`
  - [x] Span `test_fn1` appears with `component = "websocket"` attribute
  - [x] Span `test_fn2` appears as child of `test_fn1`
  - [x] Span `test_fn3` and `queue_processor_task` appear
  - [x] Events are visible (error events, custom events with attributes)
  - [x] Parent-child relationships are correct in the trace tree

**Implementation Summary:**

Updated test binary to load `.env` file and read `HYPERDX_API_KEY` from
environment:

- Added `dotenvy::dotenv().ok()` call at start of main
- Replaced hardcoded `"api-key"` with
  `std::env::var("HYPERDX_API_KEY").unwrap()`
- User confirmed traces appear correctly in HyperDX dashboard
- All test spans, events, and parent-child relationships verified working

**VERIFICATION COMPLETE**: HyperDX integration confirmed working with reference
implementation. Ready to proceed with integration into main codebase.

---

## Task 3. Create Telemetry Module

Extract telemetry setup from test binary into a proper module following our code
structure guidelines, then verify it works by refactoring the test binary to use
it.

### Part A: Create Module

- [x] Create `src/telemetry.rs` module file
- [x] Add `mod telemetry;` to `src/lib.rs`
- [x] Define `TelemetryError` enum with variants:
  - [x] `ExporterBuild(String)` - Failed to build OTLP exporter
  - [x] `ProviderSetup(String)` - Failed to setup tracer provider
  - [x] `SubscriberSetup(String)` - Failed to set global subscriber
- [x] Define `TelemetryGuard` struct:
  ```rust
  pub struct TelemetryGuard {
      tracer_provider: SdkTracerProvider,
  }
  ```
- [x] Implement `Drop` for `TelemetryGuard` that calls `force_flush()` on the
      tracer provider
- [x] Create public function with signature:
  ```rust
  pub fn setup_telemetry(
      api_key: String,
      service_name: String,
      endpoint: String
  ) -> Result<TelemetryGuard, TelemetryError>
  ```
- [x] Move batch exporter setup code from test binary into `setup_telemetry()`:
  - [x] Keep the blocking reqwest client thread spawn (critical for batch
        processor)
  - [x] Use gRPC protocol (fastest according to reference)
  - [x] Use batch config with same parameters as reference (512 batch size, 2048
        queue, 3s delay)
  - [x] Set resource attributes: `service.name` and `deployment.environment`
- [x] Replace all `.unwrap()` calls with proper error handling using `?`
      operator
- [x] Use `tracing_subscriber::layer::SubscriberExt` to add OpenTelemetry layer
      to existing subscriber
- [x] Return `TelemetryGuard` that will flush on drop
- [x] Verify module compiles: `cargo build`

### Part B: Verify by Refactoring Test Binary ✅ COMPLETED

- [x] Update `src/bin/test_hyperdx.rs` to use `st0x_hedge::setup_telemetry()`
  - [x] Replace all the manual telemetry setup code with single call to
        `setup_telemetry()`
  - [x] Keep the test spans (test_fn1, test_fn2, test_fn3) unchanged
  - [x] Store returned `TelemetryGuard` (don't let it drop early)
  - [x] Remove now-unused imports
- [x] Build and run: `cargo run --bin test_hyperdx`
- [x] **USER VERIFICATION REQUIRED**: Confirm in HyperDX dashboard:
  - [x] Service "st0x-hedge" appears (updated from "my-service-name")
  - [x] All test spans still appear (app_start, test_fn1, test_fn2, test_fn3,
        etc)
  - [x] Parent-child relationships still correct
  - [x] Events still visible

**Implementation Summary:**

Refactored test binary to use the telemetry library:

- Replaced ~90 lines of manual setup with single `setup_telemetry(api_key)` call
- Re-exported `setup_telemetry`, `TelemetryGuard`, and `TelemetryError` from
  `src/lib.rs`
- Removed unused imports (OpenTelemetry SDK types, batch processor types, etc.)
- Test spans remain unchanged and working

Fixed telemetry module to follow AGENTS.md guidelines:

- Error handling: Using `#[from]` with proper error types (`ExporterBuildError`,
  `SetGlobalDefaultError`) and `?` operator
- Imports: `SdkTracerProvider` imported and used unqualified (not
  `opentelemetry_sdk::trace::SdkTracerProvider`)
- Named constant: `TRACER_NAME` with comprehensive documentation explaining
  distinction from service name

**VERIFICATION COMPLETE**: User confirmed traces still appear correctly in
HyperDX with same 6 records.

**Design Rationale**: Extract into separate module for separation of concerns.
`TelemetryGuard` ensures graceful shutdown via RAII pattern. Proper error types
instead of unwrapping for production code. Verification step ensures no
regression.

---

## Task 4. Update Environment Configuration ✅ COMPLETED

Add HyperDX configuration fields to environment and config structs.

- [x] Update `src/env.rs` `Env` struct to add fields:
  - [x] `hyperdx_api_key: Option<String>` with `#[clap(long, env)]` annotation
  - [x] `hyperdx_service_name: String` with
        `#[clap(long, env, default_value = "st0x-hedge")]`
  - [x] `hyperdx_endpoint: String` with
        `#[clap(long, env, default_value = "https://in-otel.hyperdx.io/v1/traces")]`
- [x] Update `Config` struct to add same three fields (public visibility)
- [x] Update `Env::into_config()` method to pass through HyperDX fields
- [x] Update test helper function `create_test_config_with_order_owner()` in
      `src/env.rs` tests module:
  - [x] Set `hyperdx_api_key: None`
  - [x] Set `hyperdx_service_name: "st0x-hedge".to_string()`
  - [x] Set
        `hyperdx_endpoint: "https://in-otel.hyperdx.io/v1/traces".to_string()`
- [x] Update test helpers in other modules:
  - [x] `src/api.rs`: `create_test_config_with_mock_server()`
  - [x] `src/cli.rs`: `create_test_config_for_cli()`
- [x] Verify all code compiles: `cargo build`
- [x] Verify tests still pass: `cargo test -q --lib env`
- [x] Verify no regressions: `cargo test -q` (all 252 tests passed)
- [x] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [x] **USER VERIFICATION COMPLETE**: Confirmed test_hyperdx traces still appear
      in HyperDX (6 records)

**Implementation Summary:**

Added HyperDX configuration fields to both `Env` (CLI/environment vars) and
`Config` structs:

- `hyperdx_api_key`: Optional API key for HyperDX (telemetry is optional)
- `hyperdx_service_name`: Service name with default "st0x-hedge"
- `hyperdx_endpoint`: OTLP endpoint with default HyperDX URL

Updated all test helper functions across the codebase to include the new fields
with appropriate test values. No telemetry integration yet - this task only adds
configuration plumbing.

---

## Task 5. Integrate Telemetry Setup into Server Binary ✅ COMPLETED

Initialize telemetry in the server binary and have user verify HyperDX
connection still works.

- [x] Update `src/bin/server.rs` to conditionally setup telemetry:
  - [x] After `setup_tracing(&config.log_level)`, check if
        `config.hyperdx_api_key` is `Some`
  - [x] If API key is present, call `telemetry::setup_telemetry()` with config
        values
  - [x] Store returned `TelemetryGuard` in a variable to keep it alive for
        program lifetime
  - [x] Handle `Result` with `match` or `if let Ok` - on error, log warning and
        continue without telemetry
  - [x] Make sure guard lives until end of program (don't let it drop early)
- [x] Ensure `setup_tracing()` still works correctly alongside OpenTelemetry
      layer
- [x] Verify no regressions: `cargo test -q`
- [x] Verify HyperDX still works: `cargo run --bin test_hyperdx` (removed test
      binary)
- [x] **USER VERIFICATION COMPLETE**: test_hyperdx binary removed, verified with
      server
- [x] Build server: `cargo build --bin server`
- [x] Run server with HyperDX: `HYPERDX_API_KEY=<key> cargo run --bin server`
- [x] **USER VERIFICATION COMPLETE**: User confirmed traces appear in HyperDX
  - [x] Service "st0x-hedge" appears (not "my-service-name")
  - [x] Traces from bot operations are visible
  - [x] deployment.environment attribute is set correctly
- [x] Test server runs normally without API key: `cargo run --bin server`
      (should work without telemetry)

**VERIFICATION CHECKPOINT**: Stop here until user confirms HyperDX shows the
"st0x-hedge" service and basic traces. This confirms telemetry module
integration works before adding any instrumentation.

---

## Task 6. Add Instrumentation to Core Event Loop ✅ COMPLETED

Add tracing spans to the main event processing loop in `src/lib.rs`.

- [x] Add manual span to `launch()` function
- [x] Add `#[tracing::instrument]` to `run()` function
- [x] Add `#[tracing::instrument]` to `run_bot_session()` function
- [x] WebSocket event handling instrumented via conductor functions
- [x] Verify no regressions: `cargo test -q`
- [x] Build and run: `cargo run --bin server`
- [x] **USER VERIFICATION COMPLETE**: User confirmed traces appear correctly in
      HyperDX
  - [x] `launch` and `bot_task` spans visible
  - [x] `run` and `run_bot_session` spans appear as children
  - [x] Event processing visible through conductor spans
  - [x] Parent-child relationships are correct

**Implementation Summary:**

- `launch()`: Manual span with info_span!("launch")
- `bot_task`: Manual span wrapper for bot execution
- `run()`: Instrumented with #[tracing::instrument]
- `run_bot_session()`: Instrumented with #[tracing::instrument]
- Event processing captured through conductor function instrumentation

---

## Task 7. Add Instrumentation to Trade Processing ✅ COMPLETED

Add instrumentation to trade conversion and accumulator logic.

- [x] Add instrumentation to event conversion functions:
  - [x] `try_from_clear_v2` in src/onchain/clear.rs with tx_hash and log_index
  - [x] `try_from_take_order_if_target_owner` in src/onchain/take_order.rs
- [x] Add instrumentation to `src/onchain/accumulator.rs`:
  - [x] `process_onchain_trade` with symbol, amount, direction attributes
  - [x] `check_all_accumulated_positions` with broker_type attribute
  - [x] Events visible through existing tracing::info! calls
- [x] Verify no regressions: `cargo test -q`
- [x] Build and run with dry-run mode
- [x] **USER VERIFICATION COMPLETE**: User confirmed traces appear correctly
  - [x] Trade conversion spans visible with tx_hash/log_index
  - [x] Accumulator spans show symbol, amount, direction
  - [x] Proper parent-child relationships maintained

**Implementation Summary:**

- Event conversion: Instrumented at DEBUG level with transaction identifiers
- Trade processing: Instrumented at INFO level with trade details
- All spans use `skip_all` to avoid logging sensitive Provider data
- Existing log events provide visibility into state transitions

---

## Task 8. Add Instrumentation to Broker Integration ✅ COMPLETED

Instrument order placement and status polling in the offchain module.

- [x] Add instrumentation to `src/offchain/order_poller.rs`:
  - [x] `poll_pending_orders` at DEBUG level
  - [x] Events for polling cycles visible through existing logs
- [x] Add instrumentation to broker operations:
  - [x] `place_market_order` in crates/broker/src/schwab/broker.rs with symbol,
        shares, direction
  - [x] `get_order_status` in crates/broker/src/schwab/broker.rs with order_id
  - [x] `place_market_order` in crates/broker/src/mock.rs with symbol, shares,
        direction
- [x] Verify no regressions: `cargo test -q`
- [x] Build and run with dry-run mode
- [x] **USER VERIFICATION COMPLETE**: User confirmed traces appear correctly
  - [x] Order poller spans visible with polling frequency
  - [x] Broker operation spans show order details
  - [x] Proper parent-child relationships maintained

**Implementation Summary:**

- Order polling: Instrumented at DEBUG level (runs every 15 seconds)
- Broker operations: Instrumented at INFO level with order attributes
- Both Schwab and Mock broker implementations instrumented
- Spans capture complete order lifecycle from submission to completion

---

## Task 9. Add Instrumentation to Conductor ✅ COMPLETED

Instrument the conductor orchestration logic.

- [x] Add instrumentation to `src/conductor/mod.rs`:
  - [x] `process_next_queued_event` at DEBUG level
  - [x] `convert_event_to_trade` at DEBUG level
  - [x] `handle_filtered_event` at DEBUG level with event_id
  - [x] `process_valid_trade` at INFO level with event_id and symbol
  - [x] `execute_pending_offchain_execution` at INFO level
  - [x] `check_and_execute_accumulated_positions` at DEBUG level
- [x] Add instrumentation to `src/queue.rs`:
  - [x] `get_next_unprocessed_event` at DEBUG level
  - [x] `mark_event_processed` at DEBUG level with event_id
  - [x] `enqueue` at DEBUG level
  - [x] `enqueue_buffer` at INFO level with buffer_size
- [x] Add instrumentation to `src/onchain/backfill.rs`:
  - [x] `backfill_events_with_retry_strat` at INFO level with end_block
  - [x] `enqueue_batch_events` at DEBUG level with batch_start and batch_end
- [x] Verify no regressions: `cargo test -q`
- [x] Build and run
- [x] **USER VERIFICATION COMPLETE**: User confirmed traces appear correctly
  - [x] Conductor event processing pipeline fully visible
  - [x] Queue operations tracked with proper sequencing
  - [x] Backfill operations visible during startup

**Implementation Summary:**

- Complete instrumentation of event processing pipeline
- Queue operations tracked from enqueue → dequeue → process → mark complete
- Backfill operations visible with batch progress
- All spans use appropriate log levels (DEBUG for frequent ops, INFO for
  high-level flow)

---

## Task 10. Cleanup and Final Verification ✅ COMPLETED

Remove test binary, update documentation, run final verification.

- [x] Delete `src/bin/test_hyperdx.rs` (no longer needed)
- [x] Update `.env.example` to document HyperDX variables:
  - [x] Add `HYPERDX_API_KEY` with comment explaining optional telemetry
- [x] Add module-level doc comment to `src/telemetry.rs` explaining:
  - [x] Purpose: HyperDX trace export
  - [x] Batch exporter with blocking client requirement
  - [x] Optional: only active when API key provided
- [x] Run full verification:
  - [x] `cargo fmt` - Format code
  - [x] `cargo test -q` - All tests pass
  - [x] `pre-commit run -a` - All hooks pass
  - [x] `cargo clippy` - Static analysis passes
  - [x] `cargo build` - Build succeeds
- [x] **USER VERIFICATION COMPLETE**: End-to-end verification with HyperDX:
  - [x] Server runs with `HYPERDX_API_KEY` set
  - [x] All major flows exercised: backfill, event processing, order polling
  - [x] User confirmed in HyperDX dashboard:
    - [x] All components visible (launch, bot_task, run, conductor, accumulator,
          order_poller)
    - [x] Complete traces from event ingestion through execution
    - [x] Parent-child relationships correct throughout
    - [x] Attributes provide useful debugging information (symbol, amount,
          direction, tx_hash, etc.)

**Implementation Summary:**

- Added `HYPERDX_API_KEY` documentation to `.env.example` with explanation
- Added comprehensive module-level doc comment to `src/telemetry.rs` covering:
  - Purpose and optional nature of telemetry
  - Batch exporter architecture and configuration
  - Critical blocking HTTP client requirement and rationale
  - Usage example with error handling
  - Per-layer filtering explanation

**Final State**:

- ✅ Test binary removed
- ✅ Bot runs with full HyperDX observability when API key configured
- ✅ Bot runs without telemetry when API key not configured (fallback to
  console)
- ✅ User confirmed all application flows visible in HyperDX
- ✅ Code follows all project guidelines
- ✅ All tests and static analysis pass
