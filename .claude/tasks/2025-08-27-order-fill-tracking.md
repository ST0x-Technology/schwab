# Order Fill Tracking Implementation Plan

**Date:** 2025-08-27 **Task:** Implement order fill tracking during market hours
to capture actual execution prices for P&L calculations

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

### Section 1: Order Status Data Models

**Objective**: Create Rust structs to parse Schwab order status API responses

#### Tasks:

- [ ] Create `OrderStatus` enum (PENDING, PARTIALLY_FILLED, FILLED, CANCELLED,
      etc.)
- [ ] Create `OrderStatusResponse` struct matching OpenAPI schema
- [ ] Create `ExecutionLeg` struct for individual fill details
- [ ] Add price parsing utilities for cents conversion
- [ ] Add comprehensive unit tests for serialization/deserialization

**Design Decisions**:

- Use existing `price_cents_from_db_i64()` pattern for price handling
- Match OpenAPI field names exactly using `#[serde(rename_all = "camelCase")]`
- Handle partial fills by tracking multiple execution legs
- Validate that order IDs match expected format

### Section 2: Order Status Polling API Client

**Objective**: Implement HTTP client to fetch order status from Schwab API

#### Tasks:

- [ ] Add `get_order_status()` method to order placement module
- [ ] Implement retry logic using existing `backon::ExponentialBuilder` pattern
- [ ] Add proper error handling for 404 (order not found), 401 (auth), etc.
- [ ] Extract fill price information from `executionLegs`
- [ ] Calculate weighted average fill price for multiple executions
- [ ] Add integration tests with `httpmock` similar to existing order placement
      tests

**Design Decisions**:

- Reuse existing authentication and HTTP client patterns from
  `src/schwab/order.rs`
- Use same retry configuration as order placement (3 retries with exponential
  backoff)
- Handle market hours vs after-hours orders (different status progression)
- Support partial fills by aggregating execution leg data

### Section 3: Order Status Polling Service

**Objective**: Background service to periodically check pending orders for fills

#### Tasks:

- [ ] Create `OrderStatusPoller` struct with configurable polling interval
- [ ] Implement polling loop that queries all pending executions from database
- [ ] Add per-order polling with jittered delays to avoid API rate limits
- [ ] Update database when orders transition from PENDING to COMPLETED
- [ ] Handle edge cases: order cancellations, partial fills, order modifications
- [ ] Add comprehensive logging and error handling
- [ ] Add graceful shutdown mechanism

**Design Decisions**:

- Poll every 10-30 seconds during market hours (configurable via environment)
- Use existing `find_pending_executions_by_symbol()` to get orders needing
  status checks
- Implement jittered delays to avoid thundering herd against Schwab API
- Only poll orders placed during current trading session (avoid stale orders)
- Handle Schwab API rate limits gracefully with backoff

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

- [ ] Integrate `OrderStatusPoller` into main application startup (`src/lib.rs`)
- [ ] Run polling as background task using `tokio::spawn()`
- [ ] Add proper error handling and restart logic for poller failures
- [ ] Add health check endpoint or logging to monitor poller status
- [ ] Ensure poller respects application shutdown signals
- [ ] Add configuration options for polling behavior (interval, market hours,
      etc.)

**Design Decisions**:

- Start poller only during market hours (9:30 AM - 4:00 PM ET, configurable)
- Use existing `Env` configuration pattern for poller settings
- Coordinate with existing blockchain event processing without blocking
- Handle timezone conversion for market hours detection

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
- Test market hours boundary conditions and timezone handling

## Technical Considerations

### Market Hours Handling

- Only poll orders during active trading hours (9:30 AM - 4:00 PM ET)
- Handle extended hours trading if supported by Schwab API
- Gracefully handle market holidays and early closures

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
2. **Real-time Updates**: Orders are detected as filled within 30 seconds during
   market hours
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

This plan focuses exclusively on capturing fill prices during market hours when
orders execute nearly instantly. The polling approach is suitable for this use
case and provides the foundation for future P&L calculations.
