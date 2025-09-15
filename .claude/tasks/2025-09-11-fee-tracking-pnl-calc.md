# Fee Tracking and P&L Calculation Implementation Plan

## Overview

This task involves implementing fee tracking for Schwab trades and creating P&L
(Profit & Loss) calculation functionality. The system currently tracks trades
and executions but doesn't capture trading fees or calculate P&L.

## Design Decisions

### Fee Tracking Approach

- **Store fees at execution level**: Fees are charged per Schwab execution, not
  per onchain trade
- **Extract from OrderActivity**: Schwab API provides commission and fee data in
  the orderActivityCollection response
- **Database schema extension**: Add fee columns to schwab_executions table
  rather than creating separate tables
- **Capture all fee types**: Track commission, SEC fees, TAF fees, and other
  regulatory fees

### P&L Calculation Strategy

- **Implement as standalone module**: Create a dedicated P&L calculator that can
  be run independently
- **Support multiple cost basis methods**: Initially implement FIFO (First In,
  First Out) with architecture supporting other methods
- **Calculate both realized and unrealized P&L**: Track completed trades and
  open positions
- **Include fees in calculations**: Fees reduce realized gains or increase
  realized losses
- **Use Decimal for precision**: Avoid floating-point errors in financial
  calculations

## Implementation Tasks

### Phase 1: Research and Planning

- [x] Research Schwab API fee/commission structure
- [x] Create comprehensive implementation plan
- [x] Get plan approval from user

### Phase 2: Implementation

### Task 1: Database Schema Updates

- [x] **COMPLETED**: Database schema with individual fee tracking columns
  - commission_cents: INTEGER (nullable) - brokerage commission
  - sec_fee_cents: INTEGER (nullable) - SEC transaction fee
  - taf_fee_cents: INTEGER (nullable) - TAF (Trading Activity Fee)
  - other_fees_cents: INTEGER (nullable) - any other fees
  - Total fees calculated at query time to maintain normalization
- [x] **COMPLETED**: Update TradeState enum to include fee fields in Filled
      variant
- [x] **COMPLETED**: Update TradeStateDbFields struct to handle fee data

### Task 2: Update Order Status Response Parsing

- [x] **COMPLETED**: API structs for complex fee structure parsing
- [x] **COMPLETED**: Fee extraction methods for all fee types
- [x] **COMPLETED**: Comprehensive tests for fee parsing scenarios

### Task 3: Modify Order Poller to Save Fees (NEEDS REVISION after Task 1 & 2)

- [ ] Update order_poller.rs to extract fees when order is filled (after API
      fix)
- [ ] Modify execution update logic to include fee columns
- [ ] Ensure fees are properly persisted when transitioning to FILLED status
- [ ] Add logging for fee capture for debugging
- [ ] Handle cases where fee data might be unavailable

### Task 4: Implement P&L Calculation Module

- [ ] Create src/pnl.rs with core P&L calculation logic (single module file, not
      directory)
- [ ] Implement PnlCalculator struct with methods:
  - calculate_realized_pnl() - for closed positions
  - calculate_unrealized_pnl() - for open positions
  - calculate_position_pnl() - for specific symbol
- [ ] Create PositionLot struct to track individual purchases
- [ ] Implement FIFO matching algorithm for pairing buys and sells
- [ ] Include fee allocation in P&L calculations
- [ ] Support partial fills and position averaging

### Task 5: Add P&L Database Query Functions

- [ ] Create database query functions to retrieve:
  - All trades for a symbol in chronological order
  - All executions with fees for a symbol
  - Current position quantities
- [ ] Implement basic aggregation functions for P&L calculations

### Task 6: Create Comprehensive Tests

- [ ] Unit tests for fee parsing from API responses (in order_status.rs)
- [ ] Unit tests for FIFO matching algorithm
- [ ] Tests for fee inclusion in P&L calculations
- [ ] Edge case tests:
  - Partial fills
  - Multiple executions for same trade
  - Zero quantity positions
  - Negative P&L with fees
- [ ] Integration tests with mock Schwab API responses
- [ ] Test Decimal precision for financial calculations

## Technical Details

### Fee Data Structure from Schwab API

Reference @../../account_orders_openapi.yaml

```json
{
  "orderActivityCollection": [{
    "executionLegs": [{
      "price": 150.25,
      "quantity": 100
    }]
  }],
  "commissionAndFee": {
    "commission": {
      "commissionLegs": [{
        "commissionValues": [{
          "value": 0.65,
          "type": "COMMISSION"
        }]
      }]
    },
    "fee": {
      "feeLegs": [{
        "feeValues": [{
          "value": 0.01,
          "type": "SEC_FEE"
        }]
      }]
    }
  }
}
```

### P&L Calculation Formula

```
Realized P&L = (Sell Price - Buy Price) * Quantity - Total Fees
Unrealized P&L = (Current Price - Buy Price) * Quantity - Buy Fees
```

### FIFO Matching Example

```
Buy 100 @ $50 (Fee: $0.65)
Buy 50 @ $55 (Fee: $0.65)
Sell 120 @ $60 (Fee: $0.65)

FIFO Match:
- 100 from first buy: (60-50)*100 - fees = $1000 - fees
- 20 from second buy: (60-55)*20 - fees = $100 - fees
Remaining: 30 @ $55
```

## Testing Strategy

### Unit Test Coverage

- Fee parsing from API responses
- Cents conversion and rounding
- FIFO matching algorithm
- P&L calculation with various scenarios
- Decimal precision in calculations

### Integration Test Scenarios

1. Single buy/sell cycle with fees
2. Multiple partial fills with different fees
3. Long position with unrealized gains
4. Short position calculations
5. Mixed long/short positions
6. Zero-fee trades (if applicable)

## Success Criteria

- Fees are accurately captured from Schwab API responses
- Fees are properly stored in database with execution records
- P&L calculations correctly include fees
- FIFO matching produces accurate cost basis
- All calculations use proper decimal precision
- Comprehensive test coverage (>90%)

## Notes and Considerations

- Schwab may not provide fee data immediately; might need polling
- Different account types may have different fee structures
- All calculations must use proper financial arithmetic with decimal precision
- May need to handle corporate actions (splits, dividends) in future
