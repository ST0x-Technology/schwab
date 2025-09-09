# **Arbitrage Bot Specification for Tokenized Equities (V1 MVP)**

## **Background**

Early-stage onchain tokenized equity markets typically suffer from poor price
discovery and limited liquidity. Without sufficient market makers, onchain
prices can diverge substantially from traditional equity market prices, creating
a poor user experience and limiting adoption.

## **Solution Overview**

This specification outlines a minimum viable product (MVP) arbitrage bot that
helps establish price discovery by exploiting discrepancies between onchain
tokenized equities and their traditional market counterparts.

The bot monitors Raindex Orders from a specific owner that continuously offer
tokenized equities at spreads around Pyth oracle prices. When a solver clears
any of these orders, the bot immediately executes an offsetting trade on Charles
Schwab, maintaining market-neutral positions while capturing the spread
differential.

The focus is on getting a functional system live quickly. There are known risks
that will be addressed in future iterations as total value locked (TVL) grows
and the system proves market fit.

## **Operational Process and Architecture**

### **System Components**

**Onchain Infrastructure:**

- Raindex orderbook with deployed Orders from specific owner using Pyth oracle
  feeds
  - Multiple orders continuously offer to buy/sell different tokenized equities
    at Pyth price ± spread
- Order vaults holding stablecoins and tokenized equities

**Offchain Infrastructure:**

- Charles Schwab brokerage account with API access
- Arbitrage bot monitoring and execution engine
- Basic terminal/logging interface for system overview

**Bridge Infrastructure:**

- st0x bridge for offchain ↔ onchain asset movement

### **Operational Flow**

**Normal Operation Cycle:**

1. Orders continuously offer to buy/sell tokenized equities at Pyth price ±
   spread
2. Bot monitors Raindex for clears involving any orders from the arbitrageur's
   owner address
3. Bot records onchain trades and accumulates net position changes per symbol
4. When accumulated net position reaches ≥1 whole share, execute offsetting
   trade on Charles Schwab for the floor of the net amount, continuing to track
   the remaining fractional amount
5. Bot maintains running inventory of positions across both venues
6. Periodic rebalancing via st0x bridge to normalize inventory levels

Example (Offchain Batching):

- Onchain trades: 0.3 AAPL sold, 0.5 AAPL sold, 0.4 AAPL sold → net 1.2 AAPL
  sold
- Bot executes: Buy 1 AAPL share on Schwab (floor of 1.2), continues tracking
  0.2 AAPL net exposure
- Continue accumulating fractional amount until next whole share threshold is
  reached

**Rebalancing Process (Manual for now):**

- Monitor inventory drift over time, executing st0x bridge transfers to
  rebalance equity positions on/offchain
- Move stablecoins/USD as needed to maintain adequate trading capital
- Maintain sufficient offchain equity positions to match potential onchain sales
  and viceversa

## **Bot Implementation Specification**

The arbitrage bot will be built in Rust to leverage its performance, safety, and
excellent async ecosystem for handling concurrent trading flows.

### **Event Monitoring**

**Raindex Event Monitor:**

- WebSocket or polling connection to Ethereum node
- Filter for events involving any orders from the arbitrageur's owner address
  (Clear and TakeOrder events)
- Parse events to extract: symbol, quantity, price, direction
- Generate unique identifiers using transaction hash and log index for trade
  tracking

**Event-Driven Async Architecture:**

- Each blockchain event spawns an independent async execution flow using Rust's
  async/await
- Multiple trade flows run concurrently without blocking each other
- Handles throughput mismatch: fast onchain events vs slower Schwab
  execution/confirmation
- No artificial concurrency limits - process events as fast as they arrive
- Tokio async runtime manages hundreds of concurrent trades efficiently on
  limited hardware
- Each flow: Parse Event → Event Queue → Deduplication Check → Position
  Accumulation → Schwab Execution (when threshold reached) → Record Result
- Failed flows retry independently without affecting other trades

### **Trade Execution**

**Charles Schwab API Integration:**

- OAuth 2.0 authentication flow with token refresh
- Connection pooling and retry logic for API calls with exponential backoff
- Rate limiting compliance and queue management
- Market order execution for immediate fills
- Order status tracking and confirmation with polling
- Position querying for inventory management
- Account balance monitoring for available capital

**Idempotency Controls:**

- Event queue table to track all events with unique (transaction_hash,
  log_index) keys prevents duplicate processing
- Check event queue before processing any event to prevent duplicates
- Onchain trades are recorded immediately upon event processing
- Position accumulation happens in dedicated accumulators table per symbol
- Schwab executions track status ('PENDING', 'COMPLETED', 'FAILED') with retry
  logic
- Complete audit trail maintained linking individual trades to batch executions
- Record actual executed amounts and prices from both venues for spread analysis
- Proper error handling and structured error logging

### **Trade Tracking and Reporting**

**SQLite Trade Database:**

The bot uses a multi-table SQLite database to track trades and manage state. Key
tables include: onchain trade records, Schwab execution tracking, position
accumulators for batching fractional shares, audit trail linking, OAuth token
storage, and event queue for idempotency. The complete database schema is
defined in `migrations/20250703115746_trades.sql`.

- Store each onchain trade with symbol, amount, direction, and price
- Track Schwab executions separately with whole share amounts and status
- Accumulate fractional positions per symbol until execution thresholds are
  reached
- Maintain complete audit trail linking onchain trades to Schwab executions
- Handle concurrent database writes safely with per-symbol locking

**Reporting and Analysis:**

- Calculate profit/loss for each trade pair using actual executed amounts
- Generate running totals and performance reports over time
- Track inventory positions across both venues
- Push aggregated metrics to external logging system using structured logging
- Identify unprofitable trades for strategy optimization
- Separate reporting process reads from SQLite database for analysis without
  impacting trading performance

### **Health Monitoring and Logging**

- System uptime and connectivity status using structured logging
- API rate limiting and error tracking with metrics collection
- Position drift alerts and rebalancing triggers
- Latency monitoring for trade execution timing
- Configuration management with environment variables and config files
- Proper error propagation and custom error types

### **Risk Management**

- Manual override capabilities for emergency situations with proper
  authentication
- Graceful shutdown handling to complete in-flight trades before stopping

### **CI/CD and Deployment**

**Containerization:**

- Docker containerization for consistent deployment with multi-stage builds
- Simple CI/CD pipeline for automated builds and deployments
- Health check endpoints for container orchestration
- Environment-based configuration injection
- Resource limits and restart policies for production deployment

## **System Risks**

The following risks are known for v1 but will not be addressed in the initial
implementation. Solutions will be developed in later iterations.

### **Offchain Risks**

- **Fractional Share Exposure**: Charles Schwab API doesn't support fractional
  shares, requiring offchain batching until net positions reach whole share
  amounts. This creates temporary unhedged exposure for fractional amounts that
  haven't reached the execution threshold.
- **Missed Trade Execution**: The bot fails to execute offsetting trades on
  Charles Schwab when onchain trades occur, creating unhedged exposure. For
  example:
  - Bot downtime while onchain order remains active
  - Bot detects onchain trade but fails to execute offchain trade
  - Charles Schwab API failures or rate limiting during critical periods
- **After-Hours Trading Gap**: Pyth oracle may continue operating when
  traditional markets are closed, allowing onchain trades while Schwab markets
  are unavailable. Creates guaranteed daily exposure windows.

### **Onchain Risks**

- **Stale Pyth Oracle Data**: If the oracle becomes stale, the order won't trade
  onchain, resulting in missed arbitrage opportunities. However, this is
  preferable to the alternative scenario where trades execute onchain but the
  bot cannot make offsetting offchain trades.
- **Solver fails:** if the solver fails, again onchain trades won't happen but
  as above this is simply opportunity cost.

## **Charles Schwab Set up**

To begin arbitraging, you must first set up a Charles Schwab account. If you are
based outside of the US, please register with Charles Schwab International.

Once your trading account is established, navigate to the developer site at:
https://developer.schwab.com/

Register a new account on this site using the same details as your trading
account. After completing registration, you will see three setup options:
Individual, Company, or Join a Company. Select the option to set up as an
individual.

Next, proceed to the API Products section and choose "Individual Developers".
Click on "Trader API" and request access. In the request make sure you add your
Charles Schwab account number.

Charles Schwab will then process your request, which typically takes 3-5 days.
During this period, your developer account will be linked with your trading
account.
