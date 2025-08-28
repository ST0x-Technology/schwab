# Order Fill Tracking Implementation Plan

**Date:** 2025-08-27 **Task:** Implement order fill tracking to capture actual
execution prices for P&L calculations

## Current State Analysis

### Existing Implementation

- **Order Placement**: `src/schwab/order.rs` handles order placement and
  extracts order IDs from Location headers
- **Execution Tracking**: `src/schwab/execution.rs` manages `SchwabExecution`
  entities with PENDING/COMPLETED/FAILED states
- **Database Schema**: `schwab_executions` table tracks orders with
  `price_cents` field (currently hardcoded to 0)
- **API Spec Available**: OpenAPI spec shows order status endpoints with
  `filledQuantity` and `executionLegs` fields

### Current Limitations

- Order placement sets `price_cents: 0` (TODO comment in
  `src/schwab/order.rs:308`)
- No polling mechanism to check order fill status
- No retrieval of actual execution prices from Schwab API
- Cannot calculate accurate P&L due to missing fill price data

### Available Schwab APIs (from OpenAPI spec)

- `GET /accounts/{accountNumber}/orders/{orderId}` - Get specific order details
- `GET /accounts/{accountNumber}/orders` - Get all orders for account
- `GET /orders` - Get all orders for all accounts
- Response includes `filledQuantity`, `executionLegs` with price information

## Implementation Plan

### Section 1: Order Status Data Models ✅ COMPLETED

**Objective**: Create Rust structs to parse Schwab order status API responses

#### Tasks:

- [x] Create `OrderStatus` enum (PENDING, PARTIALLY_FILLED, FILLED, CANCELLED,
      etc.)
- [x] Create `OrderStatusResponse` struct matching OpenAPI schema
- [x] Create `ExecutionLeg` struct for individual fill details
- [x] Add price parsing utilities for cents conversion
- [x] Add comprehensive unit tests for serialization/deserialization

#### Implementation Details:

- **Created `src/schwab/order_status.rs`** with complete order status data
  models
- **`OrderStatus` enum** covers all Schwab API states (17 different statuses)
- **`ExecutionLeg` struct** handles individual fill details with execution ID,
  quantity, price, and timestamp
- **`OrderStatusResponse` struct** matches OpenAPI schema with camelCase
  serialization
- **Price conversion utilities** integrate with existing
  `price_cents_from_db_i64` pattern
- **Comprehensive test coverage** including weighted average price calculations,
  edge cases, and complex API response parsing
- **15 passing unit tests** covering serialization, deserialization, price
  calculations, and status checking

**Design Decisions**:

- Use existing `price_cents_from_db_i64()` pattern for price handling
- Match OpenAPI field names exactly using `#[serde(rename_all = "camelCase")]`
- Handle partial fills by tracking multiple execution legs
- Validate that order IDs match expected format

### Section 2: Order Status Polling API Client ✅ COMPLETED

**Objective**: Implement HTTP client to fetch order status from Schwab API

#### Tasks:

- [x] Add `get_order_status()` method to order placement module
- [x] Implement retry logic using existing `backon::ExponentialBuilder` pattern
- [x] Add proper error handling for 404 (order not found), 401 (auth), etc.
- [x] Extract fill price information from `executionLegs`
- [x] Calculate weighted average fill price for multiple executions
- [x] Add integration tests with `httpmock` similar to existing order placement
      tests

#### Implementation Details:

- **Added `Order::get_order_status()` static method** in `src/schwab/order.rs`
  that fetches order status from Schwab API
- **Integrated with existing OrderStatusResponse** from Section 1 for parsing
  API responses
- **Comprehensive error handling** for 404 (order not found), 401
  (unauthorized), 500 (server error), and invalid JSON responses
- **Retry logic** uses existing `backon::ExponentialBuilder` pattern consistent
  with order placement
- **8 comprehensive integration tests** covering success scenarios (filled,
  working, partially filled), error scenarios (not found, auth failure, server
  error), and retry behavior
- **Weighted average price calculation** leverages methods from
  OrderStatusResponse struct
- **Authentication integration** uses existing `SchwabTokens` and account hash
  retrieval patterns

**Design Decisions**:

- Reuse existing authentication and HTTP client patterns from
  `src/schwab/order.rs`
- Use same retry configuration as order placement (3 retries with exponential
  backoff)
- Handle market hours vs after-hours orders (different status progression)
- Support partial fills by aggregating execution leg data
- Follow existing test patterns with httpmock for consistent test structure

### Section 3: Order Status Polling Service ✅ COMPLETED

**Objective**: Background service to periodically check pending orders for fills

#### Tasks:

- [x] Create `OrderStatusPoller` struct with configurable polling interval
- [x] Implement polling loop that queries all pending executions from database
- [x] Add per-order polling with jittered delays to avoid API rate limits
- [x] Update database when orders transition from PENDING to COMPLETED
- [x] Handle edge cases: order cancellations, partial fills, order modifications
- [x] Add comprehensive logging and error handling
- [x] Add graceful shutdown mechanism
- [x] Integrate with real order IDs from database (not placeholder)
- [x] Fix type modeling violations:
  - [x] Refactor `TradeStatus` enum to properly represent pending-with-order-id
        state
  - [x] Make database schema align with type model (no contradictory columns)
  - [x] Eliminate need for separate database query for pending order_ids
- [x] Apply typestate pattern improvements:
  - [x] Create typestate for orders that can be polled vs those that cannot
  - [x] Use phantom types to enforce polling prerequisites at compile time
- [x] Fix code quality violations per CLAUDE.md:
  - [x] Fix visibility levels (use pub(crate) instead of pub)
  - [x] Remove excessive nesting and use functional patterns
  - [x] Fix import conventions and qualified usage
  - [x] Inline variables in macros where possible
  - [x] Replace imperative loops with iterator chains where appropriate
- [x] Remove all market hours related code (out of scope):
  - [x] Remove `market_hours_only` field from `OrderPollerConfig`
  - [x] Remove `should_poll()` method entirely
  - [x] Remove market hours check from polling loop
  - [x] Update tests to remove market hours references
- [x] Run tests to verify all changes work correctly
- [x] Run clippy and fix all warnings
- [x] Run pre-commit hooks

#### Implementation Details:

- **Created `src/schwab/order_poller.rs`** with polling service implementation
- **`OrderPollerConfig` struct** provides configurable polling behavior:
  - Default 15-second polling interval
  - Configurable jitter up to 5 seconds between orders
  - Currently includes market hours field that needs removal
- **`OrderStatusPoller` service** implements async polling loop with graceful
  shutdown
- **Database integration** uses existing
  `find_executions_by_symbol_and_status()` to find pending orders
- **Status update logic** converts order status responses to appropriate
  TradeStatus transitions
- **Graceful shutdown** via tokio watch channel for clean service termination

**Implementation Issues Found and Resolved**:

1. **Database Schema Constraint**: Original migration prevented storing order_id
   with PENDING status
   - **Resolution**: Modified migration to allow order_id in PENDING status
   - **Impact**: Required database recreation and test updates

2. **Order Workflow Change**: System was immediately marking orders as COMPLETED
   with price_cents: 0
   - **Resolution**: Modified `handle_execution_success()` to use new
     `PendingExecution` variant
   - **Impact**: All tests needed updates to expect `PendingExecution` status

3. **Real Order ID Integration**: Initial implementation used placeholder logic
   - **Resolution**: Implemented proper database query to fetch order_id from
     PENDING executions
   - **Impact**: Poller now correctly retrieves and uses real order IDs

4. **Type Modeling Violations** (**RESOLVED**):
   - **Problem**: `TradeStatus::Pending` variant didn't include order_id field,
     but database allows storing order_id with PENDING status
   - **Resolution**: Added `PendingExecution { order_id: String }` variant to
     distinguish between pre-submission and awaiting-execution states
   - **Benefit**: Type system now enforces that only orders with IDs can be
     polled, eliminating database-type mismatch

5. **Code Quality Issues** (**RESOLVED**):
   - **Fixed**: All visibility levels now use `pub(crate)` appropriately
   - **Fixed**: Removed excessive nesting, used functional patterns and early
     returns
   - **Fixed**: Applied proper clippy suggestions for match arms and let-else
     patterns
   - **Fixed**: Removed all market hours logic (out of scope)
   - **Fixed**: All tests updated and passing
   - **Fixed**: All clippy warnings resolved
   - **Fixed**: Pre-commit hooks passing

**Implemented Type Model**:

The `TradeStatus` enum now properly models order state progression:

```rust
pub enum TradeStatus {
    Pending,  // No order_id yet (pre-submission)
    Submitted { order_id: String },  // Has order_id, awaiting fill
    Filled {
        executed_at: DateTime<Utc>,
        order_id: String,
        price_cents: u64,
    },
    Failed {
        failed_at: DateTime<Utc>,
        error_reason: Option<String>,
    },
}
```

This eliminates the database-type mismatch and makes invalid states
unrepresentable. Only orders with `Submitted` status can be polled.

**Design Decisions**:

- Poll every 15 seconds by default (configurable via `OrderPollerConfig`)
- Use existing `find_executions_by_symbol_and_status()` to get pending orders
- Implement deterministic jittered delays to avoid thundering herd against
  Schwab API
- Use tokio::select! for responsive shutdown signaling
- Continue polling despite individual order failures to maintain service
  resilience
- Query database directly for order_id when processing PENDING executions

**Integration Notes**:

- **Database schema**: Modified to allow order_id storage in PENDING status
- **Order placement**: Now stores order_id in PENDING executions (not
  immediately COMPLETED)
- **Test updates**: 291 tests passing after updates to expect new workflow
- **Dead code warnings**: Expected until Section 5 integrates the poller into
  main application

### Section 4: Database Integration

**Objective**: Update existing database operations to store actual fill prices

#### Tasks:

- [ ] Modify `handle_execution_success()` in `src/schwab/order.rs` to accept
      actual price
- [ ] Update `update_execution_status_within_transaction()` to handle real price
      data
- [ ] Add database migration if needed for additional fill tracking fields
- [ ] Add audit logging for price updates (before/after values)
- [ ] Ensure atomicity between status update and price recording
- [ ] Add database constraints to prevent price_cents being NULL for COMPLETED
      status

**Design Decisions**:

- Leverage existing transaction-based update patterns in
  `src/schwab/execution.rs`
- Maintain backward compatibility with existing COMPLETED records (price_cents
  = 0)
- Use existing `price_cents_from_db_i64()` conversion utilities
- Add database-level constraints to ensure data consistency

### Section 5: Integration with Main Event Loop

**Objective**: Start order status polling alongside existing blockchain event
processing

#### Tasks:

- [ ] Remove all `#[allow(dead_code)]` annotations from Section 3 implementation
- [ ] Integrate `OrderStatusPoller` into main application startup (`src/lib.rs`)
- [ ] Run polling as background task using `tokio::spawn()`
- [ ] Add proper error handling and restart logic for poller failures
- [ ] Add health check endpoint or logging to monitor poller status
- [ ] Ensure poller respects application shutdown signals
- [ ] Add configuration options for polling behavior (interval, jitter)

**Design Decisions**:

- Run poller continuously when application is active
- Use existing `Env` configuration pattern for poller settings
- Coordinate with existing blockchain event processing without blocking

### Section 6: Testing Strategy

**Objective**: Comprehensive test coverage for fill tracking functionality

#### Tasks:

- [ ] Unit tests for order status data models with various API response
      scenarios
- [ ] Integration tests for order status API client with `httpmock`
- [ ] Database tests for price update transactions and constraints
- [ ] End-to-end tests simulating order placement → polling → fill detection
- [ ] Performance tests for polling under high order volume
- [ ] Error scenario tests (API failures, network issues, partial responses)

**Design Decisions**:

- Follow existing test patterns in `src/schwab/order.rs` and
  `src/schwab/execution.rs`
- Use in-memory SQLite for database tests to ensure isolation
- Mock various Schwab API responses (partial fills, full fills, cancellations)

## Technical Considerations

### API Rate Limiting

- Schwab API likely has rate limits - implement respectful polling intervals
- Use jittered delays between requests to different orders
- Implement exponential backoff for rate limit errors (429 responses)

### Error Recovery

- Handle transient network failures with retries
- Log and alert on persistent API failures
- Ensure database consistency during error scenarios

### Performance

- Minimize database queries by batching pending execution lookups
- Cache order status to avoid redundant API calls
- Use indexes on `schwab_executions.status` for efficient PENDING queries

### Data Consistency

- Ensure atomic updates of order status and fill price
- Handle concurrent access to same order records
- Validate fill prices are reasonable (within market bounds)

## Success Criteria

1. **Accurate Fill Prices**: All completed orders have actual execution prices
   from Schwab API
2. **Real-time Updates**: Orders are detected as filled within 30 seconds of
   execution
3. **Reliability**: System handles API failures gracefully without losing fill
   data
4. **Performance**: Polling does not impact main blockchain event processing
   performance
5. **Data Integrity**: Database constraints prevent incomplete or invalid fill
   records
6. **Observability**: Comprehensive logging allows monitoring of fill tracking
   health

## Future Extensions (Out of Scope)

- P&L calculation based on fill prices (separate implementation)
- After-hours order management and cancellation logic
- Advanced order types (limit orders, stop losses) if needed
- Real-time WebSocket updates instead of polling (if Schwab supports)
- Portfolio-level position reconciliation

## Implementation Priority

This plan focuses exclusively on capturing fill prices when orders execute. The
polling approach provides the foundation for future P&L calculations and order
management features.
