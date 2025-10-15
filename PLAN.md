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

## Task 5. Integrate Telemetry Setup into Server Binary

Initialize telemetry in the server binary and have user verify HyperDX
connection still works.

- [ ] Update `src/bin/server.rs` to conditionally setup telemetry:
  - [ ] After `setup_tracing(&config.log_level)`, check if
        `config.hyperdx_api_key` is `Some`
  - [ ] If API key is present, call `telemetry::setup_telemetry()` with config
        values
  - [ ] Store returned `TelemetryGuard` in a variable to keep it alive for
        program lifetime
  - [ ] Handle `Result` with `match` or `if let Ok` - on error, log warning and
        continue without telemetry
  - [ ] Make sure guard lives until end of program (don't let it drop early)
- [ ] Ensure `setup_tracing()` still works correctly alongside OpenTelemetry
      layer
- [ ] Verify no regressions: `cargo test -q`
- [ ] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [ ] **USER VERIFICATION REQUIRED**: Confirm test_hyperdx traces still appear
      in HyperDX (6 records)
- [ ] Build server: `cargo build --bin server`
- [ ] Run server with HyperDX: `HYPERDX_API_KEY=<key> cargo run --bin server`
- [ ] **USER VERIFICATION REQUIRED**: Ask user to check HyperDX dashboard and
      confirm:
  - [ ] Service "st0x-hedge" appears (not "my-service-name")
  - [ ] Some basic traces from the server startup are visible
  - [ ] deployment.environment attribute is set correctly
- [ ] Test server runs normally without API key: `cargo run --bin server`
      (should work without telemetry)

**VERIFICATION CHECKPOINT**: Stop here until user confirms HyperDX shows the
"st0x-hedge" service and basic traces. This confirms telemetry module
integration works before adding any instrumentation.

---

## Task 6. Add Instrumentation to Core Event Loop

Add tracing spans to the main event processing loop in `src/lib.rs`.

- [ ] Add `#[tracing::instrument(skip_all, fields(component = "main"))]` to
      `launch()` function
- [ ] Add `#[tracing::instrument(skip_all, fields(component = "event_loop"))]`
      to `run()` function
- [ ] Add manual span around WebSocket event handling in the `tokio::select!`
      branches:
  - [ ] Create span with `event_type = "ClearV2"` for clear event branch
  - [ ] Create span with `event_type = "TakeOrderV2"` for take order branch
  - [ ] Use `.instrument(span)` pattern for the async event processing
- [ ] Verify no regressions: `cargo test -q`
- [ ] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [ ] **USER VERIFICATION REQUIRED**: Confirm test_hyperdx traces still appear
      in HyperDX (6 records)
- [ ] Build and run: `HYPERDX_API_KEY=<key> cargo run --bin server`
- [ ] **USER VERIFICATION REQUIRED**: Ask user to check HyperDX and confirm:
  - [ ] `launch` span appears with `component = "main"` attribute
  - [ ] `run` span appears as child of `launch` with `component = "event_loop"`
  - [ ] Event handling spans appear when events are processed
  - [ ] Parent-child relationships are correct

**VERIFICATION CHECKPOINT**: Stop here until user confirms main event loop spans
appear in HyperDX. Test with actual blockchain events if possible.

---

## Task 7. Add Instrumentation to Trade Processing

Add instrumentation to trade conversion and accumulator logic.

- [ ] Add instrumentation to `src/onchain/trade.rs`:
  - [ ] Add `#[tracing::instrument]` to key trade conversion functions
  - [ ] Include span attributes: `symbol`, `amount`, `direction` from trade data
  - [ ] Skip large or sensitive fields with `skip` parameter
- [ ] Add instrumentation to `src/onchain/accumulator.rs`:
  - [ ] Add
        `#[tracing::instrument(skip_all, fields(component = "accumulator"))]` to
        accumulation functions
  - [ ] Add `symbol` attribute to accumulation spans
  - [ ] Add events for key state changes using `tracing::info!` or
        `span.add_event()`:
    - [ ] Trade accepted into accumulator
    - [ ] Threshold reached, execution triggered
    - [ ] Accumulator state after execution
- [ ] Verify no regressions: `cargo test -q`
- [ ] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [ ] **USER VERIFICATION REQUIRED**: Confirm test_hyperdx traces still appear
      in HyperDX (6 records)
- [ ] Build and run with test trades
- [ ] **USER VERIFICATION REQUIRED**: Ask user to trigger test trades and
      confirm in HyperDX:
  - [ ] Trade processing spans appear with correct attributes
  - [ ] Accumulator spans show component and symbol
  - [ ] Events are visible within spans
  - [ ] Spans are children of appropriate parent spans (event processing)

**VERIFICATION CHECKPOINT**: Stop here until user confirms trade processing
traces appear correctly in HyperDX with proper parent-child relationships.

---

## Task 8. Add Instrumentation to Broker Integration

Instrument order placement and status polling in the offchain module.

- [ ] Add instrumentation to `src/offchain/order_poller.rs`:
  - [ ] Add
        `#[tracing::instrument(skip_all, fields(component = "order_poller"))]`
        to polling functions
  - [ ] Add span attributes: `order_id`, `symbol`, `execution_id` where
        available
  - [ ] Add events for order state transitions:
    - [ ] Order status check started
    - [ ] Order filled (with execution price)
    - [ ] Order failed (with reason)
- [ ] Add instrumentation to order execution functions:
  - [ ] Add spans around order placement with `symbol`, `shares`, `direction`
        attributes
  - [ ] Add events when orders are submitted
- [ ] Verify no regressions: `cargo test -q`
- [ ] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [ ] **USER VERIFICATION REQUIRED**: Confirm test_hyperdx traces still appear
      in HyperDX (6 records)
- [ ] Build and run with order execution flow
- [ ] **USER VERIFICATION REQUIRED**: Ask user to execute some orders and
      confirm in HyperDX:
  - [ ] Order poller spans appear with `component = "order_poller"`
  - [ ] Order lifecycle visible: submission → polling → filled/failed
  - [ ] Attributes and events provide execution details
  - [ ] Order spans are children of appropriate trade/accumulator spans

**VERIFICATION CHECKPOINT**: Stop here until user confirms order execution
lifecycle is visible in HyperDX traces.

---

## Task 9. Add Instrumentation to Conductor

Instrument the conductor orchestration logic.

- [ ] Add instrumentation to `src/conductor/mod.rs`:
  - [ ] Add `#[tracing::instrument(skip_all, fields(component = "conductor"))]`
        to conductor functions
  - [ ] Add span attributes for queue processing: `queue_depth`,
        `events_processed`
  - [ ] Add events for conductor state changes and decisions
- [ ] Verify no regressions: `cargo test -q`
- [ ] Verify HyperDX still works: `cargo run --bin test_hyperdx`
- [ ] **USER VERIFICATION REQUIRED**: Confirm test_hyperdx traces still appear
      in HyperDX (6 records)
- [ ] Build and run
- [ ] **USER VERIFICATION REQUIRED**: Ask user to check HyperDX and confirm:
  - [ ] Conductor spans appear with `component = "conductor"` attribute
  - [ ] Queue processing visible with depth metrics
  - [ ] Conductor orchestration logic shows in trace hierarchy

**VERIFICATION CHECKPOINT**: Stop here until user confirms conductor traces
appear and show orchestration flow in HyperDX.

---

## Task 10. Cleanup and Final Verification

Remove test binary, update documentation, run final verification.

- [ ] Delete `src/bin/test_hyperdx.rs` (no longer needed)
- [ ] Update `.env.example` to document HyperDX variables:
  - [ ] Add `HYPERDX_API_KEY` with comment explaining optional telemetry
  - [ ] Add `HYPERDX_SERVICE_NAME` with comment about service identification
  - [ ] Add `HYPERDX_ENDPOINT` with comment about default endpoint
- [ ] Add module-level doc comment to `src/telemetry.rs` explaining:
  - [ ] Purpose: HyperDX trace export
  - [ ] Batch exporter with blocking client requirement
  - [ ] Optional: only active when API key provided
- [ ] Run full verification:
  - [ ] `cargo fmt` - Format code
  - [ ] `cargo test -q` - All tests pass
  - [ ] `rainix-rs-static` - Static analysis passes
  - [ ] `cargo build --release` - Release build succeeds
- [ ] **USER VERIFICATION REQUIRED**: Ask user to run end-to-end verification
      with HyperDX:
  - [ ] Start server with `HYPERDX_API_KEY` set
  - [ ] Exercise all major flows: event processing, trade execution, order
        polling
  - [ ] User checks HyperDX dashboard to confirm:
    - [ ] All components visible (main, event_loop, accumulator, order_poller,
          conductor)
    - [ ] Complete traces from event ingestion through order execution
    - [ ] Parent-child relationships correct throughout
    - [ ] Attributes and events provide useful debugging information
  - [ ] Verify server runs normally without `HYPERDX_API_KEY` (no telemetry)

**Completion Criteria**:

- Test binary removed
- Bot runs normally with full HyperDX observability when API key configured
- Bot runs normally without telemetry when API key not configured
- User confirms all application flows visible in HyperDX with proper
  instrumentation
- Code follows all project guidelines
- All tests and static analysis pass
