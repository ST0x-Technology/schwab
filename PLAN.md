# OTLP Migration Plan - Grafana Metrics Integration

## Current State (VERIFIED WORKING ✅)

- ✅ Manual HTTP export is working and committed
- ✅ Metrics appearing in Grafana: heartbeat_counter_total=1,
  token_retry_attempts_total=3, system_startup_total=1
- ✅ Clean baseline with no compilation warnings
- ✅ 30-second export interval with successful HTTP 200 responses

## Phase 2: Incremental OTLP Migration Plan

### Step 1: Add OTLP Dependencies & Runtime Fix Preparation

**Goal**: Set up OTLP dependencies while keeping manual export working

**Actions**:

1. Add back OTLP imports:
   - `use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};`
   - `use opentelemetry_sdk::metrics::PeriodicReader;`
2. Keep manual export as primary (no changes to setup flow)
3. Test: Run server, verify metrics still appear in Grafana
4. **Checkpoint**: Confirm with user that metrics still work

### Step 2: Implement Spawn Blocking OTLP Setup

**Goal**: Add OTLP setup in spawn_blocking to avoid runtime panic

**Actions**:

1. In `setup()` function, add spawn_blocking OTLP setup:
   ```rust
   let otlp_result = tokio::task::spawn_blocking({
       // Move OTLP setup here with proper error handling
   }).await;
   ```
2. Keep manual export as primary for now
3. Log whether OTLP setup succeeded or failed
4. Test: Run server, verify no runtime panic
5. **Checkpoint**: Confirm metrics still appear in Grafana

### Step 3: Try OTLP First with Manual Fallback

**Goal**: Use OTLP as primary, manual as fallback

**Actions**:

1. Modify logic to:
   - Try OTLP setup first via spawn_blocking
   - If successful, use OTLP with PeriodicReader
   - If failed, use manual HTTP export
2. Add clear logging for which method is active
3. Test: Run server, check logs for which method is used
4. **Checkpoint**: Confirm metrics appear in Grafana

### Step 4: Verify OTLP is Actually Working

**Goal**: Ensure OTLP PeriodicReader is sending metrics

**Actions**:

1. Temporarily increase export frequency (10 seconds)
2. Add debug logging in OTLP path
3. Run for 2 minutes minimum
4. **Checkpoint**: Verify continuous metric updates in Grafana

### Step 5: Clean Up Manual Export

**Goal**: Remove manual export once OTLP is proven stable

**Actions**:

1. Remove `create_manual_fallback` function
2. Remove manual export logic from setup
3. Keep OTLP as sole export method
4. Test: Run for 5 minutes
5. **Final Checkpoint**: Confirm stable metrics in Grafana

## Critical Success Criteria

- ✅ No runtime panics ("no reactor running")
- ✅ Metrics appear consistently in Grafana
- ✅ Clean code with no warnings
- ✅ OTLP PeriodicReader working properly
- ✅ Each step verified before proceeding

## Rollback Plan

At any step, if metrics stop appearing:

1. Revert to previous working state immediately
2. Debug the specific failure
3. Only proceed when issue is resolved and metrics confirmed

## Verification Process

After each step:

1. Run `cargo run --bin server -- --dry-run` for 60+ seconds
2. Check logs for successful metric export
3. Confirm with user that metrics appear in Grafana UI
4. Only proceed to next step when confirmed working

---

_This plan ensures we maintain working metrics throughout the migration to
proper OTLP implementation._
