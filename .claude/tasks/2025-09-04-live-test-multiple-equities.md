# Live Test Preparation: Multiple Equities

**Date:** 2025-09-04\
**Test Scenario:** Live test with multiple equity symbols (GME and NVDA)\
**Database State:** Fresh reset - all tables empty

## Trade Data Analysis

### GME0x Trades (Chronological Order)

All GME0x trades are SELL orders (negative output = gave away GME0x for USDC):

| Time     | Transaction Hash | Amount                | Direction | Notes         |
| -------- | ---------------- | --------------------- | --------- | ------------- |
| 11:07 AM | 0x34f...57ccf    | -0.392127850777440263 | SELL      | Largest trade |
| 11:07 AM | 0xa05...c8218    | -0.2                  | SELL      |               |
| 11:08 AM | 0x834...c5fa4    | -0.2                  | SELL      |               |
| 11:08 AM | 0x294...6313d    | -0.2                  | SELL      |               |
| 11:09 AM | 0x94c...27431    | -0.2                  | SELL      |               |
| 11:09 AM | 0x86d...7974c    | -0.2                  | SELL      |               |
| 11:10 AM | 0x3a9...be5b6    | -0.2                  | SELL      |               |
| 11:11 AM | 0x62d...6e730    | -0.2                  | SELL      |               |
| 11:11 AM | 0x687...796ad    | -0.2                  | SELL      |               |
| 11:12 AM | 0x51d...54f10    | -0.2                  | SELL      |               |
| 11:13 AM | 0xc9b...e9b04    | -0.2                  | SELL      |               |
| 11:14 AM | 0x02f...a66ae    | -0.2                  | SELL      |               |

**Total GME0x Sold:** 0.392127850777440263 + (11 × 0.2) = 2.592127850777440263
shares

### NVDAs1 Trades (Chronological Order)

All NVDAs1 trades are SELL orders (negative output = gave away NVDAs1 for USDC):

| Time     | Transaction Hash | Amount                | Direction | Notes         |
| -------- | ---------------- | --------------------- | --------- | ------------- |
| 11:25 AM | 0x844...a42d4    | -0.374110112659224827 | SELL      | Largest trade |
| 11:26 AM | 0x700...bfb85    | -0.2                  | SELL      |               |
| 11:27 AM | 0x280...1f55c    | -0.2                  | SELL      |               |
| 11:27 AM | 0xb10...4b443    | -0.2                  | SELL      |               |
| 11:33 AM | 0x727...b5cfa    | -0.23758823299632286  | SELL      |               |

**Total NVDAs1 Sold:** 0.374110112659224827 + (3 × 0.2) + 0.23758823299632286 =
1.211698345655547687 shares

## Expected Accumulator States

### GME Accumulator

- **Symbol:** GME (base symbol from GME0x)
- **Accumulated Long:** 0.0 (no BUY trades)
- **Accumulated Short:** 2.592127850777440263 (all SELL trades create short
  exposure)
- **Net Position:** 0.0 - 2.592127850777440263 = **-2.592127850777440263**
- **Execution Trigger:** |net_position| = 2.592127850777440263 ≥ 1.0 → **YES**
- **Executable Shares:** floor(2.592127850777440263) = **2 shares**
- **Execution Direction:** Net negative → Schwab **BUY** to offset short
  exposure

**Expected State After Execution:**

- **Pending Execution:** BUY 2 shares of GME
- **Remaining Accumulated Short:** 2.592127850777440263 - 2.0 =
  0.592127850777440263
- **Remaining Net Position:** -0.592127850777440263

### NVDA Accumulator

- **Symbol:** NVDA (base symbol from NVDAs1)
- **Accumulated Long:** 0.0 (no BUY trades)
- **Accumulated Short:** 1.211698345655547687 (all SELL trades create short
  exposure)
- **Net Position:** 0.0 - 1.211698345655547687 = **-1.211698345655547687**
- **Execution Trigger:** |net_position| = 1.211698345655547687 ≥ 1.0 → **YES**
- **Executable Shares:** floor(1.211698345655547687) = **1 share**
- **Execution Direction:** Net negative → Schwab **BUY** to offset short
  exposure

**Expected State After Execution:**

- **Pending Execution:** BUY 1 share of NVDA
- **Remaining Accumulated Short:** 1.211698345655547687 - 1.0 =
  0.211698345655547687
- **Remaining Net Position:** -0.211698345655547687

## Expected Schwab Executions

The bot should create the following Schwab executions:

1. **GME Execution**
   - **Action:** BUY 2 shares
   - **Symbol:** GME
   - **Shares:** 2
   - **Direction:** BUY
   - **Status:** PENDING → SUBMITTED → COMPLETED/FAILED
   - **Reason:** Offset accumulated short exposure from onchain SELL trades

2. **NVDA Execution**
   - **Action:** BUY 1 share
   - **Symbol:** NVDA
   - **Shares:** 1
   - **Direction:** BUY
   - **Status:** PENDING → SUBMITTED → COMPLETED/FAILED
   - **Reason:** Offset accumulated short exposure from onchain SELL trades

## Database Validation Commands

After running the bot, use these commands to validate the expected state:

### Check Accumulators

```sql
SELECT 
    symbol,
    accumulated_long,
    accumulated_short,
    net_position,
    pending_execution_id
FROM trade_accumulators_with_net 
ORDER BY symbol;
```

**Expected Results:**

- **GME:** accumulated_long=0.0, accumulated_short=0.592127850777440263,
  net_position=-0.592127850777440263, pending_execution_id=NOT NULL
- **NVDA:** accumulated_long=0.0, accumulated_short=0.211698345655547687,
  net_position=-0.211698345655547687, pending_execution_id=NOT NULL

### Check Executions

```sql
SELECT 
    id,
    symbol,
    shares,
    direction,
    status
FROM schwab_executions 
ORDER BY symbol;
```

**Expected Results:**

- **GME:** shares=2, direction='BUY', status='PENDING'/'SUBMITTED'/'COMPLETED'
- **NVDA:** shares=1, direction='BUY', status='PENDING'/'SUBMITTED'/'COMPLETED'

### Check Trade Count

```sql
SELECT COUNT(*) as total_trades FROM onchain_trades;
```

**Expected Result:** 17 trades total (12 GME + 5 NVDA)

### Check Trade-Execution Links

```sql
SELECT 
    execution_id,
    COUNT(*) as linked_trades,
    SUM(contributed_shares) as total_contribution
FROM trade_execution_links 
GROUP BY execution_id
ORDER BY execution_id;
```

**Expected Results:**

- **GME Execution:** linked_trades=12, total_contribution=2.0 (all GME trades
  contribute)
- **NVDA Execution:** linked_trades=5, total_contribution=1.0 (all NVDA trades
  contribute)

## Test Execution Steps

1. **Database Reset Verification**
   ```bash
   cargo run --bin cli -- view-accumulators
   cargo run --bin cli -- view-executions
   cargo run --bin cli -- view-trades
   ```
   All should show empty results.

2. **Run Bot with Test Data**
   - Start the bot with the onchain event feed
   - Monitor logs for trade processing and execution creation
   - Watch for exactly 2 executions (1 GME, 1 NVDA)

3. **Validate Final State**
   - Run all validation SQL queries above
   - Verify accumulator states match expected values
   - Confirm both executions were created with correct parameters

## Edge Cases to Monitor

1. **Execution Order**
   - GME and NVDA executions may be created in different orders depending on
     trade processing sequence
   - Both should eventually be created regardless of order

2. **Fractional Remainder Handling**
   - GME should have ~0.592 shares remaining in accumulated_short
   - NVDA should have ~0.212 shares remaining in accumulated_short
   - These fractional amounts should NOT trigger additional executions

3. **Trade-Execution Linkage**
   - All 12 GME trades should contribute to the GME execution
   - All 5 NVDA trades should contribute to the NVDA execution
   - No cross-symbol contamination should occur

4. **Symbol Mapping**
   - GME0x → GME (0x suffix stripped)
   - NVDAs1 → NVDA (s1 suffix should be stripped to NVDA base symbol)

5. **Concurrent Processing**
   - Multiple trades for the same symbol may be processed concurrently
   - Symbol locking should prevent duplicate executions
   - Final accumulator state should be consistent regardless of processing order

## Success Criteria

✅ **17 onchain trades saved** (12 GME + 5 NVDA)\
✅ **2 Schwab executions created** (2×GME BUY, 1×NVDA BUY)\
✅ **Correct accumulator states** (fractional remainders as calculated)\
✅ **Complete audit trail** (all trades linked to appropriate executions)\
✅ **No duplicate executions** (exactly 1 execution per symbol despite multiple
triggering trades)\
✅ **Proper symbol mapping** (0x and s1 suffixes handled correctly)

## Failure Scenarios to Debug

❌ **No executions created** → Check symbol extraction logic and threshold
calculations\
❌ **Wrong execution counts** → Check concurrent processing and locking
mechanisms\
❌ **Incorrect accumulator states** → Check trade direction mapping and
calculation logic\
❌ **Missing trade-execution links** → Check linkage creation in chronological
order\
❌ **Symbol mapping errors** → Check base symbol extraction from tokenized
symbols

## Task 7: Fix Excessive Logging Issues

### Problem Summary

The live test revealed excessive and redundant logging that makes console output
hard to read, especially:

- Multiple consecutive "market is closed" logs during startup
- Hundreds of debug logs during backfill processing that flood the console
- Log lines that are too wide for a single screen
- Redundant market status checks causing duplicate logging

### Root Cause Analysis

**Logging Issues Identified:**

- `src/lib.rs:143-145`: Redundant `should_bot_run()` check before
  `wait_until_market_open()`
- `src/onchain/backfill.rs:52-54`: Debug log printed for every 1000-block batch
  (268+ logs)
- `src/onchain/backfill.rs:22`: `BACKFILL_BATCH_SIZE = 1000` is too small,
  causing excessive batches
- `src/trading_hours_controller.rs:55,59`: Duplicate "Market is closed" debug
  logs
- Market hours cache logs appear in quick succession during startup

### Implementation Checklist

- [ ] **Remove redundant market status check in main loop**:
  - [ ] Update `src/lib.rs:143-145` to remove duplicate `should_bot_run()` call
        before `wait_until_market_open()`
- [ ] **Reduce backfill logging verbosity**:
  - [ ] Increase `BACKFILL_BATCH_SIZE` from 1000 to 10000 blocks in
        `src/onchain/backfill.rs:22`
  - [ ] Remove or reduce frequency of debug log on
        `src/onchain/backfill.rs:52-54`
  - [ ] Add single progress log showing percentage completion instead of
        per-batch logs
- [ ] **Fix duplicate market status logging**:
  - [ ] Remove duplicate "Market is closed" debug log in
        `src/trading_hours_controller.rs:59`
  - [ ] Keep only the INFO level log when market is closed
- [ ] **Improve cache logging**:
  - [ ] Change market hours cache hit/miss logs from DEBUG to TRACE level
- [ ] **Test logging improvements**:
  - [ ] Run bot during market closed hours to verify cleaner logging output
  - [ ] Run bot during market open with backfill to verify reduced log volume

### Expected Outcomes

- **Market closed state**: Single clear message instead of 4+ consecutive logs
- **Backfill processing**: ~27 log lines instead of 268+ for same block range
- **Overall verbosity**: Significantly cleaner console output during both
  startup and operation
- **Readability**: Log lines that fit within standard terminal width

## Task 8: Add Support for s1 Suffix Alongside 0x

### Problem Summary

NVDA trades failed validation because the bot only accepts "0x" suffixed
tokenized equity symbols, but legacy symbols use "s1" suffix. For backward
compatibility, both suffixes must be supported.

**Error from logs**:
`"Expected IO to contain USDC and one 0x-suffixed symbol but got USDC and NVDAs1"`

### Root Cause Analysis

**Suffix validation hardcoded to "0x" only in multiple locations:**

- `src/error.rs:23-24`: Error message mentions "0x-suffixed symbol"
- `src/onchain/trade.rs:248,258`: `ends_with("0x")` checks
- `src/onchain/trade.rs:274-288`: `extract_ticker_from_0x_symbol` function
  strips only "0x"
- `src/onchain/accumulator.rs:163-165`: `extract_base_symbol` strips only "0x"
- `src/onchain/accumulator.rs:323`: Reconstructs with "0x" suffix only
- `src/onchain/trade_execution_link.rs:164`: Uses "0x" suffix check
- `src/cli.rs:475`: Uses "0x" suffix check

### Implementation Checklist

- [ ] **Update suffix validation in trade processing**:
  - [ ] Replace `ends_with("0x")` with `(ends_with("0x") || ends_with("s1"))` in
        `src/onchain/trade.rs:248,258`
  - [ ] Update `extract_ticker_from_0x_symbol` to handle both "0x" and "s1"
        suffix stripping
  - [ ] Update function name to `extract_ticker_from_tokenized_symbol` for
        clarity
- [ ] **Update accumulator logic**:
  - [ ] Update `extract_base_symbol` in `src/onchain/accumulator.rs:163-165` to
        strip both suffixes
  - [ ] Update symbol reconstruction on line 323 to preserve original suffix
        type
- [ ] **Update other validation locations**:
  - [ ] Update suffix check in `src/onchain/trade_execution_link.rs:164` to
        accept both suffixes
  - [ ] Update suffix check in `src/cli.rs:475` to accept both suffixes
- [ ] **Update error messages**:
  - [ ] Change error message in `src/error.rs:23-24` to mention "0x or
        s1-suffixed symbol"
- [ ] **Add comprehensive tests**:
  - [ ] Test GME0x symbol processing (existing behavior)
  - [ ] Test NVDAs1 symbol processing (new behavior)
  - [ ] Test mixed scenarios with both suffix types
  - [ ] Update existing test names that reference "0x" specifically
- [ ] **Verify end-to-end functionality**:
  - [ ] Run live test with both GME0x and NVDAs1 symbols
  - [ ] Confirm both symbols create successful Schwab executions
  - [ ] Verify accumulator correctly handles both suffix types

### Expected Outcomes

- **GME0x trades**: Continue processing as before (backward compatibility
  maintained)
- **NVDAs1 trades**: Successfully process without validation errors
- **Mixed symbol support**: Handle portfolios containing both "0x" and "s1"
  suffixed symbols
- **Error clarity**: Updated error messages reflect support for both suffix
  types
