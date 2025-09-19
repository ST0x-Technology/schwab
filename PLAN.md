# OTLP Migration Plan - Grafana Metrics Integration

## Current State (VERIFIED WORKING ✅)

- ✅ Manual HTTP export is working and committed
- ✅ Metrics appearing in Grafana: heartbeat_counter_total=1,
  token_retry_attempts_total=3, system_startup_total=1
- ✅ Clean baseline with no compilation warnings
- ✅ 10-second export interval with successful HTTP 200 responses
- ✅ Improved error handling using `?` operator and `let-else` patterns

## Phase 2: Initial OTLP Migration Attempt (COMPLETED WITH LEARNINGS)

### Task 1: Add OTLP Dependencies & Runtime Fix Preparation

- [x] Add back OTLP imports: `WithExportConfig`, `WithHttpConfig`,
      `PeriodicReader`
- [x] Keep manual export as primary (no changes to setup flow)
- [x] Test: Run server, verify metrics still appear in Grafana
- [x] **Checkpoint**: Confirmed with user that metrics still work

### Task 2: Implement Spawn Blocking OTLP Setup

- [x] Add spawn_blocking OTLP setup in `setup()` function
- [x] Keep manual export as primary for testing
- [x] Log whether OTLP setup succeeded or failed
- [x] Test: Run server, verify no runtime panic initially
- [x] **Checkpoint**: Confirmed metrics still appear in Grafana

### Task 3: Try OTLP First with Manual Fallback

- [x] Modify logic to try OTLP setup first via spawn_blocking
- [x] Set global meter provider when OTLP succeeds
- [x] Add clear logging for which method is active
- [x] Test: Run server, check logs for which method is used
- [x] **Issue Discovered**: OTLP setup succeeded but metrics not reaching
      Grafana

### Task 4: Verify OTLP is Actually Working

- [x] Increase export frequency to 10 seconds
- [x] Add debug logging in OTLP path
- [x] Run for 2+ minutes testing
- [x] **Critical Finding**: OTLP PeriodicReader still causes runtime panic "no
      reactor running"
- [x] **User Verification**: Confirmed metrics from OTLP attempt not appearing
      in Grafana

### Task 5: Stabilize with Working Solution

- [x] Revert to manual HTTP export due to OTLP runtime issues
- [x] Keep 10-second interval (improvement over original 30 seconds)
- [x] Clean up unused OTLP imports and dependencies
- [x] Fix error handling with `?` operator and `let-else` patterns
- [x] **Checkpoint**: Confirmed stable metrics appearing in Grafana with fresh
      timestamps

## Key Learnings from Phase 2

### Issues Identified

- ❌ **Runtime Panic**: PeriodicReader creates background threads outside Tokio
  context
- ❌ **spawn_blocking Insufficient**: Does not fully resolve the reactor issue
- ❌ **Silent Failure**: OTLP setup can succeed but fail to deliver metrics
- ❌ **Incomplete Understanding**: Need deeper research into OpenTelemetry
  architecture

### Successes

- ✅ **Manual HTTP Export**: Reliable fallback that works perfectly
- ✅ **OTLP Format**: Confirmed we can generate correct OTLP JSON
- ✅ **10-second Exports**: Improved frequency from 30 seconds
- ✅ **Verification Process**: Always check with user before claiming success

## Phase 3: Deep Research & Proper OTLP Implementation

### Task 6: Research OpenTelemetry Architecture

- [ ] Study OpenTelemetry SDK documentation for runtime requirements
- [ ] Research alternative to PeriodicReader (push vs pull exporters)
- [ ] Investigate proper Tokio runtime integration patterns
- [ ] Examine successful OTLP implementations in other Rust projects
- [ ] **Goal**: Understand root cause of "no reactor running" issue

### Task 7: Alternative OTLP Approaches

- [ ] Research `opentelemetry_sdk::export::metrics::PushController`
- [ ] Investigate manual metric export without PeriodicReader
- [ ] Test `opentelemetry::global::meter_provider()` setup patterns
- [ ] Explore different runtime configurations (Tokio vs blocking)
- [ ] **Goal**: Find OTLP approach that works with our Tokio setup

### Task 8: Protocol-Level Investigation

- [ ] Compare our manual OTLP JSON with library-generated OTLP
- [ ] Test library's HTTP client configuration options
- [ ] Investigate different OTLP transport options (HTTP vs gRPC)
- [ ] Research Grafana Cloud OTLP endpoint requirements
- [ ] **Goal**: Ensure library can generate compatible OTLP format

### Task 9: Runtime Environment Analysis

- [ ] Profile our Tokio runtime setup in `src/lib.rs`
- [ ] Test OTLP in isolated minimal examples
- [ ] Compare with official OpenTelemetry Rust examples
- [ ] Investigate if our async context is missing something
- [ ] **Goal**: Identify what our runtime needs for OTLP compatibility

### Task 10: Implementation & Integration

- [ ] Implement researched OTLP solution
- [ ] Add comprehensive error handling and logging
- [ ] Test with manual export as verified fallback
- [ ] Gradual rollout with user verification at each step
- [ ] **Goal**: Working OTLP implementation using the library properly

## Success Criteria

- ✅ No runtime panics ("no reactor running")
- ✅ Metrics appear consistently in Grafana using OTLP library
- ✅ Clean code with no warnings
- ✅ Proper OpenTelemetry SDK usage following best practices
- ✅ Each step verified with user confirmation
- ✅ Maintain manual export as fallback during transition

## Research Resources

- OpenTelemetry Rust SDK documentation
- GitHub examples and successful implementations
- Grafana Cloud OTLP documentation
- Tokio runtime integration guides
- Community discussions on similar issues

---

_Phase 3 focuses on proper research and understanding before implementation,
ensuring we leverage the OpenTelemetry library correctly rather than working
around its limitations._
