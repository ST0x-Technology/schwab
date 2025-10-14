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

## Task 1. Create Test Binary with Reference Implementation

Create a new test binary containing the EXACT reference implementation to
establish a verified baseline.

- [ ] Create `src/bin/test_hyperdx.rs`
- [ ] Copy the exact `main.rs` code from the gist into `test_hyperdx.rs`
- [ ] Keep `IS_BATCH_EXPORTER = true` (use batch exporter as reference does)
- [ ] Update workspace `Cargo.toml` dependencies to add/update:
  - [ ] Add `gzip` and `blocking` features to existing `reqwest` workspace
        dependency
  - [ ] Add `opentelemetry = "0.30.0"` to workspace dependencies
  - [ ] Add
        `opentelemetry_sdk = { version = "0.30.0", features = ["rt-tokio"] }` to
        workspace dependencies
  - [ ] Add `opentelemetry-otlp = "0.30.0"` to workspace dependencies
  - [ ] Add `tracing-opentelemetry = "0.31.0"` to workspace dependencies
- [ ] Make sure main package references these workspace dependencies
- [ ] Verify binary compiles: `cargo build --bin test_hyperdx`

**Why exact copy**: We need to establish a known-working baseline before making
any modifications. The reference implementation has specific details (blocking
client, thread spawn, gRPC protocol) that are critical for HyperDX to work.

---

## Task 2. Verify Test Binary Works with HyperDX

Run the test binary and have the user confirm traces appear in HyperDX
dashboard.

- [ ] Replace hardcoded `"api-key"` string in test binary with actual API key
      from environment variable
- [ ] Run test binary: `HYPERDX_API_KEY=<real-key> cargo run --bin test_hyperdx`
- [ ] Wait for the 10 second sleep to complete
- [ ] **USER VERIFICATION REQUIRED**: Ask user to check HyperDX dashboard and
      confirm:
  - [ ] Service "my-service-name" appears
  - [ ] Root span `app_start` is visible with `work_units = 2` attribute
  - [ ] Child span `app_start_child` appears under `app_start`
  - [ ] Span `test_fn1` appears with `component = "websocket"` attribute
  - [ ] Span `test_fn2` appears as child of `test_fn1`
  - [ ] Span `test_fn3` and `queue_processor_task` appear
  - [ ] Events are visible (error events, custom events with attributes)
  - [ ] Parent-child relationships are correct in the trace tree

**STOP HERE**: Do not proceed until user confirms HyperDX traces are working. If
traces don't appear, debug the test binary first before any integration work.

---

## Task 3. Create Telemetry Module

Extract telemetry setup from test binary into a proper module following our code
structure guidelines.

- [ ] Create `src/telemetry.rs` module file
- [ ] Add `mod telemetry;` to `src/lib.rs`
- [ ] Define `TelemetryError` enum with variants:
  - [ ] `ExporterBuild(String)` - Failed to build OTLP exporter
  - [ ] `ProviderSetup(String)` - Failed to setup tracer provider
  - [ ] `SubscriberSetup(String)` - Failed to set global subscriber
- [ ] Define `TelemetryGuard` struct:
  ```rust
  pub struct TelemetryGuard {
      tracer_provider: SdkTracerProvider,
  }
  ```
- [ ] Implement `Drop` for `TelemetryGuard` that calls `force_flush()` on the
      tracer provider
- [ ] Create public function with signature:
  ```rust
  pub fn setup_telemetry(
      api_key: String,
      service_name: String,
      endpoint: String
  ) -> Result<TelemetryGuard, TelemetryError>
  ```
- [ ] Move batch exporter setup code from test binary into `setup_telemetry()`:
  - [ ] Keep the blocking reqwest client thread spawn (critical for batch
        processor)
  - [ ] Use gRPC protocol (fastest according to reference)
  - [ ] Use batch config with same parameters as reference (512 batch size, 2048
        queue, 3s delay)
  - [ ] Set resource attributes: `service.name` and `deployment.environment`
- [ ] Replace all `.unwrap()` calls with proper error handling using `?`
      operator
- [ ] Use `tracing_subscriber::layer::SubscriberExt` to add OpenTelemetry layer
      to existing subscriber
- [ ] Return `TelemetryGuard` that will flush on drop
- [ ] Verify module compiles: `cargo build`

**Design Rationale**: Extract into separate module for separation of concerns.
`TelemetryGuard` ensures graceful shutdown via RAII pattern. Proper error types
instead of unwrapping for production code.

---

## Task 4. Update Environment Configuration

Add HyperDX configuration fields to environment and config structs.

- [ ] Update `src/env.rs` `Env` struct to add fields:
  - [ ] `hyperdx_api_key: Option<String>` with `#[clap(long, env)]` annotation
  - [ ] `hyperdx_service_name: String` with
        `#[clap(long, env, default_value = "st0x-hedge")]`
  - [ ] `hyperdx_endpoint: String` with
        `#[clap(long, env, default_value = "https://in-otel.hyperdx.io/v1/traces")]`
- [ ] Update `Config` struct to add same three fields
- [ ] Update `Env::into_config()` method to pass through HyperDX fields
- [ ] Update test helper function `create_test_config()` in `src/env.rs` tests
      module:
  - [ ] Set `hyperdx_api_key: None`
  - [ ] Set `hyperdx_service_name: "st0x-hedge".to_string()`
  - [ ] Set
        `hyperdx_endpoint: "https://in-otel.hyperdx.io/v1/traces".to_string()`
- [ ] Update `create_test_config_with_order_owner()` helper similarly
- [ ] Verify all code compiles: `cargo build`
- [ ] Verify tests still pass: `cargo test -q --lib env`

**No HyperDX integration yet** - this task only adds configuration plumbing
without making any telemetry calls.

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
