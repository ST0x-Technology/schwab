# Grafana Metrics Integration

## Objective

Integrate OpenTelemetry metrics collection into the Schwab arbitrage bot to
monitor trading operations and send metrics to Grafana Cloud for visualization
and alerting.

## Background

We have an existing `src/metrics.rs` module from another project that needs to
be adapted for this bot. The metrics will help monitor:

- Blockchain event processing
- Schwab order execution
- Token refresh operations
- Queue depth and processing
- Position accumulation
- System performance

## Design Decisions

### 1. Optional Metrics

Metrics will be completely optional - the bot will run normally if OTLP endpoint
is not configured. This ensures backward compatibility and allows running in
environments without metrics infrastructure.

### 2. Service Naming

Use "schwarbot" as the service name to match the deployment container name for
consistency in Grafana dashboards.

### 3. Environment Detection

Use the existing `dry_run` flag to determine deployment environment:

- `dry_run = true` → `deployment.environment = "dev"`
- `dry_run = false` → `deployment.environment = "prod"`

### 4. Metrics Architecture

- Store metrics in `Arc<Option<Metrics>>` for thread-safe optional access
- Pass metrics reference through background task builders
- Use structured attributes (labels) for filtering in Grafana

## Implementation Plan

### Task 1. Add OpenTelemetry Dependencies

- [x] Run `cargo add opentelemetry --features metrics,otel_unstable`
- [x] Run `cargo add opentelemetry_sdk --features rt-tokio,metrics`
- [x] Run `cargo add opentelemetry-otlp --features http-proto,reqwest-client`
- [x] Verify dependencies compile correctly

### Task 2. Update Environment Configuration

- [ ] Add optional OTLP fields to `src/env.rs`:
  - `otel_exporter_otlp_endpoint: Option<String>`
  - `otel_exporter_otlp_basic_auth_token: Option<String>`
- [ ] Add clap attributes with `env` flag for environment variable support
- [ ] Update test helpers to handle optional metrics fields
- [ ] Add method to check if metrics are configured

### Task 3. Adapt Metrics Module

- [ ] Update `src/metrics.rs`:
  - Change service name from "degen" to "schwarbot"
  - Make setup return `Option<Metrics>` (None if endpoint not configured)
  - Set deployment environment based on `dry_run` flag
  - Update imports to use `opentelemetry` instead of `opentelemetry as otlp`
- [ ] Define bot-specific metrics:
  - `onchain_events_received`: Counter with `event_type` label
  - `schwab_orders_executed`: Counter with `status`, `symbol`, `direction`
    labels
  - `token_refreshes`: Counter with `result` label
  - `queue_depth`: Gauge for pending events
  - `accumulated_positions`: Gauge with `symbol` label
  - `trade_execution_duration_ms`: Histogram for timing
- [ ] Add graceful error handling for missing configuration
- [ ] Implement Drop trait for proper shutdown

### Task 4. Initialize Metrics at Startup

- [ ] Add `pub mod metrics;` to `src/lib.rs`
- [ ] Initialize metrics in `launch` function before starting tasks
- [ ] Store as `Arc<Option<Metrics>>` in shared state
- [ ] Pass metrics to `BackgroundTasksBuilder`
- [ ] Add metrics to background task structs
- [ ] Handle metrics flush on shutdown in ctrl-c handler

### Task 5. Instrument Event Processing (`src/conductor.rs`)

- [ ] Add metrics field to `BackgroundTasksBuilder`
- [ ] Increment `onchain_events_received` counter when events arrive:
  - Label "ClearV2" for clear events
  - Label "TakeOrderV2" for take order events
- [ ] Update `queue_depth` gauge in queue processor
- [ ] Track event processing start time for duration metrics
- [ ] Record processing duration in histogram

### Task 6. Instrument Trade Execution (`src/schwab/broker.rs`)

- [ ] Add metrics parameter to broker trait methods
- [ ] In `Schwab::execute_order`:
  - Record start time for duration tracking
  - Increment `schwab_orders_executed` with "pending" status
  - On success: increment with "success" status and record duration
  - On failure: increment with "failed" status
  - Add symbol and direction as labels
- [ ] Pass metrics through to execution functions

### Task 7. Instrument Position Management (`src/onchain/accumulator.rs`)

- [ ] Add metrics parameter to accumulator functions
- [ ] Update `accumulated_positions` gauge when positions change:
  - Set gauge value to current position for each symbol
  - Update on both accumulation and clearing
- [ ] Track when positions are cleared (counter)

### Task 8. Instrument Token Management (`src/schwab/tokens.rs`)

- [ ] Add metrics parameter to token refresh functions
- [ ] Increment `token_refreshes` counter:
  - Label "success" on successful refresh
  - Label "failed" on refresh failure
  - Label "expired" when token is expired
  - Label "proactive" for proactive refreshes

### Task 9. Add Tests

- [ ] Test metrics initialization with valid config
- [ ] Test metrics initialization returns None without config
- [ ] Test counter increments in mocked scenarios
- [ ] Test gauge updates
- [ ] Test histogram recording
- [ ] Verify no panics when metrics is None

### Task 10. Integration Testing

- [ ] Run bot without OTLP config - verify normal operation
- [ ] Run bot with invalid OTLP endpoint - verify graceful degradation
- [ ] Run bot with valid test endpoint - verify metrics are sent
- [ ] Test all instrumented code paths
- [ ] Verify performance impact is minimal

### Task 11. Final Validation

- [ ] Run `cargo test -q`
- [ ] Run `cargo clippy --all-targets --all-features -- -D clippy::all`
- [ ] Run `pre-commit run -a`
- [ ] Test in dry_run mode
- [ ] Document any new environment variables in README if needed

## Testing Strategy

### Unit Tests

- Metrics module initialization with/without configuration
- Individual metric increments and updates
- Thread safety of metric operations
- Graceful handling of None metrics

### Integration Tests

- End-to-end event processing with metrics
- Trade execution with metrics recording
- Background tasks with metrics instrumentation
- Shutdown behavior with pending metrics

### Manual Testing

1. Run without OTLP configuration - verify normal operation
2. Run with test Grafana endpoint - verify metrics appear
3. Process test trades - verify all metrics update
4. Force token refresh - verify token metrics
5. Test graceful shutdown - verify metrics are flushed

## Rollback Plan

If metrics integration causes issues:

1. Set OTLP endpoint environment variables to empty
2. Bot will run without metrics (returns None from setup)
3. If critical issues, revert the metrics module changes

## Success Criteria

- Bot runs normally without OTLP configuration
- When configured, metrics appear in Grafana Cloud
- All critical code paths are instrumented
- No performance degradation (< 1% overhead)
- Clean shutdown with metrics flushing
- All tests pass including new metric tests

## Notes

- Metrics are completely optional and non-invasive
- Use structured attributes for better Grafana filtering
- Follow OpenTelemetry naming conventions (lowercase_with_underscores)
- Service name "schwarbot" matches container deployment name
- Consider adding more metrics in future based on operational needs
