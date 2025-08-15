# **Arbitrage Bot Specification for Tokenized Equities (V1 MVP)**

## **Background**

Early-stage onchain tokenized equity markets typically suffer from poor price discovery and limited liquidity. Without sufficient market makers, onchain prices can diverge substantially from traditional equity market prices, creating a poor user experience and limiting adoption.

## **Solution Overview**

This specification outlines a minimum viable product (MVP) arbitrage bot that helps establish price discovery by exploiting discrepancies between onchain tokenized equities and their traditional market counterparts.

The bot monitors a Raindex Order that continuously offers tokenized equities at spreads around Pyth oracle prices. When a solver clears this order, the bot immediately executes an offsetting trade on Charles Schwab, maintaining market-neutral positions while capturing the spread differential.

The focus is on getting a functional system live quickly. There are known risks that will be addressed in future iterations as total value locked (TVL) grows and the system proves market fit.

## **Operational Process and Architecture**

### **System Components**

**Onchain Infrastructure:**

* Raindex orderbook with deployed Order using Pyth oracle feeds  
  * Continuously offers to buy/sell tokenized equities at Pyth price ± spread  
* Order vaults holding stablecoins and tokenized equities

**Offchain Infrastructure:**

* Charles Schwab brokerage account with API access  
* Arbitrage bot monitoring and execution engine  
* Basic terminal/logging interface for system overview

**Bridge Infrastructure:**

* st0x bridge for offchain ↔ onchain asset movement

### **Operational Flow**

**Normal Operation Cycle:**

1. Order continuously offers to buy/sell tokenized equities at Pyth price ± spread  
2. Bot monitors Raindex for clears involving the arbitrageur's order  
3. Bot records onchain trades and accumulates net position changes per symbol  
4. When accumulated net position reaches ≥1 whole share, execute offsetting trade on Charles Schwab for the floor of the net amount, continuing to track the remaining fractional amount  
5. Bot maintains running inventory of positions across both venues  
6. Periodic rebalancing via st0x bridge to normalize inventory levels

Example (Offchain Batching):

* Onchain trades: 0.3 AAPL sold, 0.5 AAPL sold, 0.4 AAPL sold → net 1.2 AAPL sold  
* Bot executes: Buy 1 AAPL share on Schwab (floor of 1.2), continues tracking 0.2 AAPL net exposure  
* Continue accumulating fractional amount until next whole share threshold is reached

**Rebalancing Process (Manual for now):**

* Monitor inventory drift over time, executing st0x bridge transfers to rebalance equity positions on/offchain  
* Move stablecoins/USD as needed to maintain adequate trading capital  
* Maintain sufficient offchain equity positions to match potential onchain sales and viceversa

## **Bot Implementation Specification**

The arbitrage bot will be built in Rust to leverage its performance, safety, and excellent async ecosystem for handling concurrent trading flows.

### **Event Monitoring**

**Raindex Event Monitor:**

* WebSocket or polling connection to Ethereum node  
* Filter for events involving arbitrageur's Order (Clear and TakeOrder events)  
* Parse events to extract: symbol, quantity, price, direction  
* Generate unique identifiers using transaction hash and log index for trade tracking

**Event-Driven Async Architecture:**

* Each blockchain event spawns an independent async execution flow using Rust's async/await  
* Multiple trade flows run concurrently without blocking each other  
* Handles throughput mismatch: fast onchain events vs slower Schwab execution/confirmation  
* No artificial concurrency limits \- process events as fast as they arrive  
* Tokio async runtime manages hundreds of concurrent trades efficiently on limited hardware  
* Each flow: Parse Event → SQLite Dedupe Check → Schwab API Call → Record Result  
* Failed flows retry independently without affecting other trades

### **Trade Execution**

**Charles Schwab API Integration:**

* OAuth 2.0 authentication flow with token refresh  
* Connection pooling and retry logic for API calls with exponential backoff  
* Rate limiting compliance and queue management  
* Market order execution for immediate fills  
* Order status tracking and confirmation with polling  
* Position querying for inventory management  
* Account balance monitoring for available capital

**Idempotency Controls:**

* SQLite database to track all trade attempts with unique (transaction\_hash, log\_index) keys  
* Check database before executing any trade to prevent duplicates  
* Store trade as 'pending' before Schwab API call, update to 'completed' after execution  
* Retry failed trades during operation with exponential backoff, but require manual review for pending trades after bot restart  
* Record actual executed amounts and prices from both venues for spread analysis  
* Proper error handling and structured error logging

### **Trade Tracking and Reporting**

**SQLite Trade Database:**

* Store each trade with onchain input/output symbols and amounts  
* Record actual Schwab execution amounts and prices including fees  
* Calculate input/output ratios for both venues to analyze spreads  
* Track trade status (pending/completed/failed) with timestamps  
* Link trades to onchain events via transaction hash and log index  
* Handle concurrent database writes safely

```sql
trades (
  id: primary key
  tx_hash: text
  log_index: integer
  
  onchain_input_symbol: text
  onchain_input_amount: decimal
  onchain_output_symbol: text  
  onchain_output_amount: decimal
  onchain_io_ratio: decimal
  
  schwab_input_symbol: text
  schwab_input_amount: decimal
  schwab_output_symbol: text
  schwab_output_amount: decimal  
  schwab_io_ratio: decimal
  
  status: text
  schwab_order_id: text
  created_at: timestamp
  completed_at: timestamp
)
```

**Reporting and Analysis:**

* Calculate profit/loss for each trade pair using actual executed amounts  
* Generate running totals and performance reports over time  
* Track inventory positions across both venues  
* Push aggregated metrics to external logging system using structured logging  
* Identify unprofitable trades for strategy optimization  
* Separate reporting process reads from SQLite database for analysis without impacting trading performance

### **Health Monitoring and Logging**

* System uptime and connectivity status using structured logging  
* API rate limiting and error tracking with metrics collection  
* Position drift alerts and rebalancing triggers  
* Latency monitoring for trade execution timing  
* Configuration management with environment variables and config files  
* Proper error propagation and custom error types

### **Risk Management**

* Manual override capabilities for emergency situations with proper authentication  
* Graceful shutdown handling to complete in-flight trades before stopping

### **CI/CD and Deployment**

**Containerization:**

* Docker containerization for consistent deployment with multi-stage builds  
* Simple CI/CD pipeline for automated builds and deployments  
* Health check endpoints for container orchestration  
* Environment-based configuration injection  
* Resource limits and restart policies for production deployment

## **System Risks**

The following risks are known for v1 but will not be addressed in the initial implementation. Solutions will be developed in later iterations.

### **Offchain Risks**

* **Fractional Share Exposure**: Charles Schwab API doesn't support fractional shares, requiring offchain batching until net positions reach whole share amounts. This creates temporary unhedged exposure for fractional amounts that haven't reached the execution threshold.
* **Missed Trade Execution**: The bot fails to execute offsetting trades on Charles Schwab when onchain trades occur, creating unhedged exposure. For example:  
  * Bot downtime while onchain order remains active  
  * Bot detects onchain trade but fails to execute offchain trade  
  * Charles Schwab API failures or rate limiting during critical periods  
* **After-Hours Trading Gap**: Pyth oracle may continue operating when traditional markets are closed, allowing onchain trades while Schwab markets are unavailable. Creates guaranteed daily exposure windows.

### **Onchain Risks**

* **Stale Pyth Oracle Data**: If the oracle becomes stale, the order won't trade onchain, resulting in missed arbitrage opportunities. However, this is preferable to the alternative scenario where trades execute onchain but the bot cannot make offsetting offchain trades.  
* **Solver fails:** if the solver fails, again onchain trades won't happen but as above this is simply opportunity cost.

## **Charles Schwab Set up** 

To begin arbitraging, you must first set up a Charles Schwab account. If you are based outside of the US, please register with Charles Schwab International.

Once your trading account is established, navigate to the developer site at: https://developer.schwab.com/

Register a new account on this site using the same details as your trading account. After completing registration, you will see three setup options: Individual, Company, or Join a Company. Select the option to set up as an individual.

Next, proceed to the API Products section and choose "Individual Developers". Click on "Trader API" and request access. In the request make sure you add your Charles Schwab account number.

Charles Schwab will then process your request, which typically takes 3-5 days. During this period, your developer account will be linked with your trading account.