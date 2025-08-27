# Codebase Guidelines Violations and Improvement Plan

## Overview

This document outlines violations of our newly established coding guidelines and
provides a comprehensive improvement plan. The analysis covers type modeling,
functional programming, commenting practices, and other code quality
improvements.

## Summary of Findings

Based on the codebase review, I identified several key areas that violate our
new guidelines:

### Critical Issues (High Priority)

1. **Database State Contradictions**: Database fields can contradict enum
   variants
2. **Mixed Persistence/Domain Logic**: Structs mixing persisted and
   non-persisted state

### Medium Priority Issues

3. **Functional Programming**: Missing opportunities for iterator chains
4. **Comments**: Several violations of new commenting guidelines

### Low Priority Issues

5. **Deep Nesting**: Minimal issues found - codebase already follows good
   patterns

## Implementation Plan

### Phase 1: Critical Type Modeling Issues

#### Task 1: Fix Database State Contradictions

- [ ] Address `TradeStatus` representation issues in `src/schwab/execution.rs`
  - [ ] Fix `row_to_execution` function where database fields can contradict
        enum variants
  - [ ] Review lines 26-66 where status parsing can create inconsistent states
  - [ ] Add validation that COMPLETED status always has order_id and price_cents
  - [ ] Add validation that FAILED status always has executed_at timestamp
  - [ ] Consider using JSON column for status-specific data to align with enum
        structure
- [ ] Add database constraints to prevent invalid combinations
  - [ ] Add CHECK constraints to ensure COMPLETED rows have required fields
  - [ ] Add CHECK constraints to ensure FAILED rows have failure timestamp
  - [ ] Create migration script to add these constraints safely
- [ ] Separate database entities from domain objects
  - [ ] Consider creating separate `SchwabExecutionRow` for database operations
  - [ ] Keep `SchwabExecution` as pure domain object
  - [ ] Add clear conversion patterns between persistence and domain layers

#### Task 2: Fix Mixed Persisted/Non-Persisted State

- [ ] Address `id: Option<i64>` pattern in domain structs
  - [ ] Review `OnchainTrade` and `SchwabExecution` structs
  - [ ] Consider separate types for "new" vs "persisted" entities
  - [ ] Use typestate pattern: `Entity<New>` vs `Entity<Persisted>`
  - [ ] Update save methods to return persisted entities with guaranteed IDs

#### Task 3: Implement Typestate for Complex Operations

- [ ] Add typestate for OAuth flow in `src/schwab/mod.rs`
  - [ ] Create `OAuthFlow<State>` to prevent using expired codes
  - [ ] Define states: `Initial`, `CodeReceived { code: String }`,
        `Complete { tokens: SchwabTokens }`
  - [ ] Implement state transition methods that consume and return new states
  - [ ] Update `run_oauth_flow` to use typestate pattern
- [ ] Evaluate execution lifecycle typestate needs
  - [ ] Assess if current `TradeStatus` enum is sufficient
  - [ ] Only add typestate if it prevents real bugs or improves API safety

### Phase 2: Functional Programming Improvements

#### Task 4: Replace Imperative Patterns with Functional Equivalents

- [ ] Refactor trade allocation loop in `src/onchain/accumulator.rs`
  - [ ] Replace imperative loop in `create_trade_execution_linkages` (lines
        279-306)
  - [ ] Use `scan` combinator to track remaining shares
  - [ ] Use `take_while` to stop when allocation is complete
  - [ ] Example transformation:
    ```rust
    // Instead of imperative loop
    let allocations: Vec<TradeAllocation> = trade_rows
        .into_iter()
        .scan(remaining_execution_shares, |remaining, row| {
            if *remaining <= 0.001 { return None; }
            let available = row.trade_amount - row.already_allocated.unwrap_or(0.0);
            if available <= 0.001 { return Some(None); }
            let contribution = available.min(*remaining);
            *remaining -= contribution;
            Some(Some(TradeAllocation { trade_id: row.trade_id, contribution }))
        })
        .flatten()
        .collect();
    ```
  - [ ] Add unit tests to verify functional equivalent produces same results
  - [ ] Benchmark to ensure no performance regression
- [ ] Consider adding itertools dependency for enhanced combinators
  - [ ] Evaluate if `itertools::process_results` would improve error handling
        patterns
  - [ ] Only add dependency if it significantly improves code readability

### Phase 3: Code Quality and Documentation

#### Task 5: Remove Comment Violations

- [ ] Remove redundant comments that restate obvious code in `src/conductor.rs`
  - [ ] Remove "Save values for logging before the trade is moved" (restates
        obvious assignment)
  - [ ] Remove "Continue processing other events even if one fails" (obvious
        from continue statement)
  - [ ] Remove "Begin atomic transaction to ensure..." (obvious from
        transaction.begin())
  - [ ] Remove "Step 1:", "Step 2:" etc. comments that just mark code sections
  - [ ] Remove "Collect all unprocessed events" (obvious from the query)
- [ ] Preserve valuable business logic comments
  - [ ] Keep AfterClear event explanation in `src/onchain/clear.rs` (lines
        45-48)
  - [ ] Keep algorithm rationale comments that explain "why" not "what"
  - [ ] Keep domain-specific rule explanations
- [ ] Review and improve existing doc comments
  - [ ] Ensure public functions have proper doc comments
  - [ ] Add examples for complex API usage where helpful

#### Task 6: Final Code Review and Testing

- [ ] Add comprehensive tests for type safety improvements
  - [ ] Test that invalid database states are rejected by validation
  - [ ] Test all typestate transitions work correctly
  - [ ] Add integration tests for OAuth flow typestate
- [ ] Create safe migration path for database changes
  - [ ] Write migration scripts that add constraints safely
  - [ ] Test migrations on copy of production data structure
  - [ ] Plan rollback strategy for constraint additions
- [ ] Performance validation
  - [ ] Benchmark functional programming changes
  - [ ] Ensure typestate patterns don't add runtime overhead
  - [ ] Profile database constraint impact

## Detailed Analysis

### Database State Contradiction Issues

**Current Problem in `src/schwab/execution.rs`:**

```rust
// ❌ Database can store contradictory state
// COMPLETED status in DB but missing order_id
let parsed_status = match status {
    "COMPLETED" => {
        let order_id = order_id.ok_or_else(|| /* error */)?; // Can fail!
        TradeStatus::Completed { order_id, /* ... */ }
    }
    // ...
}
```

**Proposed Solution:**

```rust
// ✅ Database constraints prevent invalid states
// + JSON column or separate tables for status-specific data
// + Validation at database level, not just application level
```

### Functional Programming Improvements

**Current Imperative Pattern:**

```rust
// ❌ Manual state management in accumulator.rs
for row in trade_rows {
    if remaining_execution_shares <= 0.001 {
        break;
    }
    let available_amount = row.trade_amount - row.already_allocated.unwrap_or(0.0);
    if available_amount <= 0.001 {
        continue;
    }
    let contribution = available_amount.min(remaining_execution_shares);
    // ... manual state updates
    remaining_execution_shares -= contribution;
}
```

**Proposed Functional Pattern:**

```rust
// ✅ Iterator combinators with clear data flow
let allocations: Vec<_> = trade_rows
    .into_iter()
    .scan(remaining_shares, |remaining, row| {
        // Clear functional transformation
    })
    .flatten()
    .collect();
```

### Comment Violations Found

**Comments to Remove (restate obvious code):**

- "Save values for logging before the trade is moved"
- "Continue processing other events even if one fails"
- "Begin atomic transaction to ensure both trade saving and event marking happen
  together"
- Various "Step N:" section markers

**Good Comments to Preserve:**

- AfterClear event lookup algorithm explanation
- Business rule explanations like trade direction mapping
- Non-obvious domain constraints and validations

## Benefits Expected

1. **Type Safety**: Database constraints prevent invalid states at persistence
   layer
2. **API Safety**: Typestate prevents misuse of complex operations like OAuth
3. **Maintainability**: Functional patterns make data transformations more
   explicit
4. **Code Clarity**: Removing redundant comments improves signal-to-noise ratio

## Risk Assessment

- **Low Risk**: Comment cleanup and simple functional programming improvements
- **Medium Risk**: Database constraint additions (require careful migration)
- **High Risk**: Major typestate changes (only implement where clearly
  beneficial)

This plan focuses on the actual guideline violations found without
over-engineering solutions that don't add meaningful value.
