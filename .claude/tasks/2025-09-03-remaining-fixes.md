# 2025-09-03 Remaining Fixes

This file documents the remaining unresolved issues from the previous planning
files (2025-09-01 and 2025-09-02) that need to be addressed.

## Task 1: Complete BackgroundTasksBuilder Refactoring

### Problem Summary

The uncommitted changes show a partially implemented BackgroundTasksBuilder
pattern that needs to be completed. The current changes are in an incomplete
state and may not compile properly.

### Implementation Checklist

- [ ] Review and complete the uncommitted changes to `src/conductor.rs`
- [ ] Review and complete the uncommitted changes to `src/lib.rs`
- [ ] Ensure the BackgroundTasksBuilder pattern is properly implemented
- [ ] Fix any compilation issues
- [ ] Run tests to ensure everything passes (should be 331 tests)
- [ ] Commit the completed refactoring

### Background

The changes implement a builder pattern for BackgroundTasks that allows better
dependency injection and cleaner service management. This refactoring was part
of the unified event processing work.

## Task 2: Fix Accumulator Triggering Logic (HIGH PRIORITY)

### Problem Summary

**CRITICAL ISSUE**: The accumulator is triggering offsetting trades incorrectly.
According to the live testing notes:

> "The offsetting sell was triggered when accumulated long was above 1 instead
> of net being abs >= 1"

This is a financial logic bug that will cause incorrect trade executions.

### Root Cause Analysis Needed

- [ ] Review accumulator logic in `src/onchain/accumulator.rs`
- [ ] Find where the triggering condition is implemented
- [ ] Identify if it's checking `accumulated_long > 1` instead of
      `abs(net_position) >= 1`
- [ ] Understand why this logic is wrong

### Implementation Checklist

- [ ] Locate the incorrect triggering condition in accumulator code
- [ ] Fix the condition to use `abs(net_position) >= 1`
- [ ] Add test coverage for the correct behavior
- [ ] Verify the fix with unit tests
- [ ] Document the change

### Why This Is Critical

This bug affects what offsetting trades get placed, which directly impacts
financial accuracy. Must be fixed before re-running the bot in production.

## Task 3: Complete Testing & Deployment

### Problem Summary

The unified event processing system has been implemented but needs final
validation in a live environment.

### Implementation Checklist

- [ ] Run backfill to reprocess trades 19-22 that were cleaned up in the
      database
- [ ] Monitor live system logs for:
  - [ ] No UNIQUE constraint violations
  - [ ] Trades 1-18 detected as duplicates and skipped gracefully
  - [ ] Trades 19-22 processed successfully when rebackfilled
  - [ ] Appropriate Schwab executions triggered for trades 19-22
- [ ] Verify final database state matches expectations
- [ ] Document results

### Success Criteria

- All historical events process before live events
- No duplicate insert errors in logs
- Trades 19-22 trigger 1-2 Schwab executions as expected
- Accumulator shows balanced state
- System continues processing live events normally

## Task 4: Investigate Backfill Regression

### Problem Summary

From live testing notes:

> "There were 10 backfilled trades for some reason (strange regression as it
> used to work previously)"

This suggests the backfill logic may have been affected by recent changes.

### Investigation Checklist

- [ ] Review recent changes to backfill logic
- [ ] Check if event processing changes affected backfill behavior
- [ ] Verify backfill cutoff logic is working correctly
- [ ] Ensure backfill doesn't re-process already processed events
- [ ] Document findings and fix if needed

## Task 5: Fix Auth "test_auth_code" Issue (LOW PRIORITY)

### Problem Summary

From Task 15 in the old planning file:

- "test_auth_code" appears in token refresh logs in production
- This suggests test data contamination in the auth flow
- Auth CLI command was returning 400 bad request

### Investigation Checklist

- [ ] Search codebase for "test_auth_code" references
- [ ] Check where test auth code might be persisting in production
- [ ] Review auth flow for test data contamination
- [ ] Fix auth CLI command 400 bad request error
- [ ] Ensure proper auth flow in production

### Priority

Low priority since this doesn't appear to be affecting core trading
functionality, but should be addressed for production cleanliness.

## Implementation Order

1. **Task 1**: Complete BackgroundTasksBuilder refactoring (foundation work)
2. **Task 2**: Fix accumulator triggering logic (CRITICAL - must be done before
   running bot)
3. **Task 3**: Complete testing and deployment (validate the fixes)
4. **Task 4**: Investigate backfill regression (system reliability)
5. **Task 5**: Fix auth issue (cleanup, low priority)

## Notes

- Task 2 is blocking for production deployment
- All other tasks can be done after Task 2 is complete
- The unified event processing work from 2025-09-02 appears to be largely
  complete and working (331 tests passing)
