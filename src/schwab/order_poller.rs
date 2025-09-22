use chrono::Utc;
use rand::Rng;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Interval, interval};
use tracing::{debug, error, info};

use super::broker::Broker;
use super::execution::{
    find_execution_by_id, find_executions_by_symbol_and_status,
    update_execution_status_within_transaction,
};
use super::order::Order;
use super::{SchwabAuthEnv, SchwabError, TradeState};
use super::{SchwabAuthEnv, SchwabError, TradeState, TradeStatus};
use crate::lock::{clear_execution_lease, clear_pending_execution_id};

#[derive(Debug, Clone)]
pub struct OrderPollerConfig {
    pub polling_interval: Duration,
    pub max_jitter: Duration,
}

impl Default for OrderPollerConfig {
    fn default() -> Self {
        Self {
            polling_interval: Duration::from_secs(15),
            max_jitter: Duration::from_secs(5),
        }
    }
}

pub(crate) struct OrderStatusPoller<B: Broker> {
    config: OrderPollerConfig,
    env: SchwabAuthEnv,
    pool: SqlitePool,
    interval: Interval,
    shutdown_rx: watch::Receiver<bool>,
    broker: B,
}

impl<B: Broker> OrderStatusPoller<B> {
    pub(crate) fn new(
        config: OrderPollerConfig,
        env: SchwabAuthEnv,
        pool: SqlitePool,
        shutdown_rx: watch::Receiver<bool>,
        broker: B,
    ) -> Self {
        let interval = interval(config.polling_interval);

        Self {
            config,
            env,
            pool,
            interval,
            shutdown_rx,
            broker,
        }
    }
}

impl<B: Broker> OrderStatusPoller<B> {
    pub(crate) async fn run(mut self) -> Result<(), SchwabError> {
        info!(
            "Starting order status poller with interval: {:?}",
            self.config.polling_interval
        );

        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    if let Err(e) = self.poll_pending_orders().await {
                        error!("Polling cycle failed: {e}");
                    }
                }
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        info!("Received shutdown signal, stopping order poller");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn poll_pending_orders(&self) -> Result<(), SchwabError> {
        debug!("Starting polling cycle for submitted orders");

        let submitted_executions =
            find_executions_by_symbol_and_status(&self.pool, "", "SUBMITTED")
                .await
                .map_err(|e| {
                    error!("Failed to query pending executions: {e}");
                    SchwabError::InvalidConfiguration(
                        "Failed to query pending executions from database".to_string(),
                    )
                })?;

        if submitted_executions.is_empty() {
            debug!("No submitted orders to poll");
            return Ok(());
        }

        info!("Polling {} submitted orders", submitted_executions.len());

        for execution in submitted_executions {
            if *self.shutdown_rx.borrow() {
                info!("Shutdown signal received, stopping polling");
                break;
            }

            let Some(execution_id) = execution.id else {
                continue;
            };

            if let Err(e) = self.poll_execution_status(&execution).await {
                error!("Failed to poll execution {execution_id}: {e}");
            }

            self.add_jittered_delay().await;
        }

        debug!("Completed polling cycle");
        Ok(())
    }

    async fn poll_execution_status(
        &self,
        execution: &crate::schwab::execution::SchwabExecution,
    ) -> Result<(), SchwabError> {
        let Some(execution_id) = execution.id else {
            error!("Execution missing ID: {execution:?}");
            return Ok(());
        };

        let order_id = match &execution.state {
            TradeState::Pending => {
                debug!("Execution {execution_id} is PENDING but no order_id yet");
                return Ok(());
            }
            TradeState::Submitted { order_id } | TradeState::Filled { order_id, .. } => {
                order_id.clone()
            }
            TradeState::Failed { .. } => {
                debug!("Execution {execution_id} already failed, skipping poll");
                return Ok(());
            }
        };

        let order_status = self
            .broker
            .get_order_status(&order_id, &self.env, &self.pool)
            .await?;

        if order_status.is_filled() {
            self.handle_filled_order(execution_id, &order_status)
                .await?;
        } else if order_status.is_terminal_failure() {
            self.handle_failed_order(execution, &order_status).await?;
        } else {
            debug!(
                "Order {order_id} (execution {execution_id}) still pending with state: {:?}",
                order_status.status
            );
        }

        Ok(())
    }

    async fn handle_filled_order(
        &self,
        execution_id: i64,
        order_status: &super::order_status::OrderStatusResponse,
    ) -> Result<(), SchwabError> {
        let price_cents = order_status
            .price_in_cents()
            .map_err(|e| {
                error!("Failed to convert price to cents for execution {execution_id}: {e}");
                SchwabError::InvalidConfiguration("Price conversion failed".to_string())
            })?
            .ok_or_else(|| {
                error!("Filled order missing execution price for execution {execution_id}");
                SchwabError::InvalidConfiguration("Missing execution price".to_string())
            })?;

        let new_status = TradeState::Filled {
            executed_at: Utc::now(),
            order_id: order_status
                .order_id
                .clone()
                .unwrap_or_else(|| format!("UNKNOWN_{execution_id}")),
            price_cents,
        };

        let mut tx = self.pool.begin().await?;

        // Get the symbol from the execution before updating status
        let execution = find_execution_by_id(&self.pool, execution_id)
            .await
            .map_err(|e| {
                error!("Failed to find execution {execution_id}: {e}");
                SchwabError::InvalidConfiguration("Database query failed".to_string())
            })?
            .ok_or_else(|| {
                error!("Execution {execution_id} not found in database");
                SchwabError::InvalidConfiguration("Execution not found".to_string())
            })?;

        update_execution_status_within_transaction(&mut tx, execution_id, new_status).await?;

        // Clear pending execution ID and execution lease to unblock future executions
        clear_pending_execution_id(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                SchwabError::InvalidConfiguration("Invalid symbol in execution".to_string())
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear pending execution ID for symbol {}: {e}",
                execution.symbol
            );
            SchwabError::InvalidConfiguration("Failed to clear pending execution ID".to_string())
        })?;

        clear_execution_lease(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                SchwabError::InvalidConfiguration("Invalid symbol in execution".to_string())
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear execution lease for symbol {}: {e}",
                execution.symbol
            );
            SchwabError::InvalidConfiguration("Failed to clear execution lease".to_string())
        })?;

        tx.commit().await?;

        info!(
            "Updated execution {execution_id} to FILLED with price: {} cents and cleared locks for symbol: {}",
            price_cents, execution.symbol
        );

        Ok(())
    }

    async fn handle_failed_order(
        &self,
        execution_id: i64,
        order_status: &super::order_status::OrderStatusResponse,
    ) -> Result<(), SchwabError> {
        let new_status = TradeState::Failed {
            failed_at: Utc::now(),
            error_reason: Some(format!("Order state: {:?}", order_status.status)),
        };

        let mut tx = self.pool.begin().await?;

        // Get the symbol from the execution before updating status
        let execution = find_execution_by_id(&self.pool, execution_id)
            .await
            .map_err(|e| {
                error!("Failed to find execution {execution_id}: {e}");
                SchwabError::InvalidConfiguration("Database query failed".to_string())
            })?
            .ok_or_else(|| {
                error!("Execution {execution_id} not found in database");
                SchwabError::InvalidConfiguration("Execution not found".to_string())
            })?;

        update_execution_status_within_transaction(&mut tx, execution_id, new_status).await?;

        // Clear pending execution ID and execution lease to unblock future executions
        clear_pending_execution_id(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                SchwabError::InvalidConfiguration("Invalid symbol in execution".to_string())
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear pending execution ID for symbol {}: {e}",
                execution.symbol
            );
            SchwabError::InvalidConfiguration("Failed to clear pending execution ID".to_string())
        })?;

        clear_execution_lease(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                SchwabError::InvalidConfiguration("Invalid symbol in execution".to_string())
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear execution lease for symbol {}: {e}",
                execution.symbol
            );
            SchwabError::InvalidConfiguration("Failed to clear execution lease".to_string())
        })?;

        tx.commit().await?;

        info!(
            "Updated execution {execution_id} to FAILED due to order status: {:?} and cleared locks for symbol: {}",
            order_status.status, execution.symbol
        );

        Ok(())
    }

    async fn add_jittered_delay(&self) {
        if self.config.max_jitter > Duration::ZERO {
            #[allow(clippy::cast_possible_truncation)]
            let max_jitter_millis = self.config.max_jitter.as_millis() as u64;
            let jitter_millis = rand::thread_rng().gen_range(0..max_jitter_millis);
            let jitter = Duration::from_millis(jitter_millis);
            tokio::time::sleep(jitter).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schwab::Direction;
    use crate::schwab::TradeStatus;
    use crate::schwab::broker::Schwab;
    use crate::schwab::execution::SchwabExecution;
    use crate::test_utils::setup_test_db;
    use httpmock::Mock;
    use httpmock::prelude::*;
    use serde_json::json;
    use tokio::sync::watch;

    #[tokio::test]
    async fn test_order_poller_config_default() {
        let config = OrderPollerConfig::default();
        assert_eq!(config.polling_interval, Duration::from_secs(15));
        assert_eq!(config.max_jitter, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_order_poller_creation() {
        let config = OrderPollerConfig::default();
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
            account_index: 0,
        };
        let pool = setup_test_db().await;
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let poller = OrderStatusPoller::new(config.clone(), env, pool, shutdown_rx, Schwab);
        assert_eq!(poller.config.polling_interval, config.polling_interval);
        assert_eq!(poller.config.max_jitter, config.max_jitter);
    }

    #[tokio::test]
    async fn test_poll_pending_orders_empty_database() {
        let config = OrderPollerConfig::default();
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
            account_index: 0,
        };
        let pool = setup_test_db().await;
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let poller = OrderStatusPoller::new(config, env, pool, shutdown_rx, Schwab);

        let result = poller.poll_pending_orders().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_poll_execution_status_missing_order_id() {
        let config = OrderPollerConfig::default();
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: "https://api.schwabapi.com".to_string(),
            account_index: 0,
        };
        let pool = setup_test_db().await;
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            state: TradeState::Pending,
        };

        let mut tx = pool.begin().await.unwrap();
        let execution_id = execution.save_within_transaction(&mut tx).await.unwrap();
        tx.commit().await.unwrap();

        let poller = OrderStatusPoller::new(config, env, pool.clone(), shutdown_rx, Schwab);

        // Fetch the execution to pass to poll_execution_status
        let execution = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        let result = poller.poll_execution_status(&execution).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn test_end_to_end_order_flow() {
        let server = MockServer::start();
        let pool = setup_test_db().await;

        // Setup test environment with mock server
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: server.base_url(),
            account_index: 0,
        };

        // Setup test tokens in database
        let tokens = crate::schwab::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(&pool).await.unwrap();

        // Mock account hash endpoint
        let account_mock = server.mock(|when, then| {
            when.method(GET).path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        // Step 1: Create a SchwabExecution directly (simulating the result of onchain trade processing)
        // This reflects the real architecture where executions are created from onchain trades
        let execution = SchwabExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            state: TradeState::Submitted {
                order_id: "1004055538999".to_string(),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        // Step 2: Verify execution was saved to database with SUBMITTED status
        let saved_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "AAPL",
            TradeStatus::Submitted,
        )
        .await
        .unwrap();
        assert_eq!(saved_executions.len(), 1);
        let saved_execution = &saved_executions[0];
        assert_eq!(saved_execution.shares, 100);
        assert_eq!(saved_execution.direction, Direction::Buy);
        assert!(matches!(
            &saved_execution.state,
            TradeState::Submitted { order_id } if order_id == "1004055538999"
        ));

        // Step 3: Mock order status polling with sequence - first WORKING, then FILLED
        let order_status_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/1004055538999")
                .header("authorization", "Bearer test_access_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "orderId": 1_004_055_538_123_i64,
                    "status": "FILLED",
                    "filledQuantity": 100.0,
                    "remainingQuantity": 0.0,
                    "enteredTime": "2023-10-15T10:25:00Z",
                    "closeTime": "2023-10-15T10:30:00Z",
                    "orderActivityCollection": [{
                        "activityType": "EXECUTION",
                        "executionLegs": [{
                            "executionId": "EXEC123",
                            "quantity": 100.0,
                            "price": 150.25,
                            "time": "2023-10-15T10:30:00Z"
                        }]
                    }]
                }));
        });

        // Step 4: Poll for status and let the poller find it's filled
        let config = OrderPollerConfig::default();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let poller = OrderStatusPoller::new(config, env.clone(), pool.clone(), shutdown_rx, Schwab);

        // Step 5: Poll for status and verify order gets updated to FILLED with actual price
        let execution = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        let poll_result = poller.poll_execution_status(&execution).await;

        assert!(poll_result.is_ok());

        // Step 6: Verify final state - order should be FILLED with actual execution price
        let final_execution = crate::schwab::execution::find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            &final_execution.state,
            TradeState::Filled { order_id, price_cents, .. }
            if order_id == "1004055538123" && *price_cents == 15025  // 150.25 * 100 cents
        ));

        // Step 7: Verify no more SUBMITTED executions for this symbol
        let submitted_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "AAPL",
            TradeStatus::Submitted,
        )
        .await
        .unwrap();
        assert_eq!(submitted_executions.len(), 0);

        // Step 8: Verify there is now one FILLED execution
        let filled_executions =
            find_executions_by_symbol_and_status(&pool, "AAPL", TradeStatus::Filled)
                .await
                .unwrap();
        assert_eq!(filled_executions.len(), 1);
        assert_eq!(filled_executions[0].id, Some(execution_id));

        // Step 9: Verify pending_execution_id was cleared from trade_accumulators
        let accumulator_row = sqlx::query!(
            "SELECT pending_execution_id FROM trade_accumulators WHERE symbol = ?1",
            "AAPL"
        )
        .fetch_optional(&pool)
        .await
        .unwrap();

        // If accumulator exists, pending_execution_id should be NULL
        if let Some(row) = accumulator_row {
            assert_eq!(
                row.pending_execution_id, None,
                "pending_execution_id should be cleared after order fills"
            );
        }

        // Step 10: Verify symbol lock was cleared from symbol_locks
        let lock_count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM symbol_locks WHERE symbol = ?1",
            "AAPL"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            lock_count, 0,
            "Symbol lock should be cleared after order fills"
        );

        // Verify all mocks were called as expected
        account_mock.assert_hits(1); // Called during polling
        order_status_mock.assert();

        // Trigger shutdown for clean test completion
        shutdown_tx.send(true).unwrap();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::cast_precision_loss)]
    async fn test_high_volume_order_polling_performance() {
        use std::time::Instant;

        let server = MockServer::start();
        let pool = setup_test_db().await;

        // Setup test environment
        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: server.base_url(),
            account_index: 0,
        };

        // Setup test tokens
        let tokens = crate::schwab::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(&pool).await.unwrap();

        // Mock account hash endpoint
        let account_mock = server.mock(|when, then| {
            when.method(GET).path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        // Create many executions to poll (simulate high volume scenario)
        let num_orders = 50;
        let mut execution_ids = Vec::new();

        for i in 0..num_orders {
            let execution = SchwabExecution {
                id: None,
                symbol: format!("TEST{i}"), // Unique symbol for each execution
                shares: 100 + (i * 10) as u64, // Varying share amounts
                direction: if i % 2 == 0 {
                    Direction::Buy
                } else {
                    Direction::Sell
                },
                state: TradeState::Submitted {
                    order_id: format!("ORDER{i:04}"),
                },
            };

            let mut sql_tx = pool.begin().await.unwrap();
            let execution_id = execution
                .save_within_transaction(&mut sql_tx)
                .await
                .unwrap();
            sql_tx.commit().await.unwrap();
            execution_ids.push(execution_id);

            // Mock the order status response for this execution
            let order_id = format!("ORDER{i:04}");
            let price = (i as f64).mul_add(0.25, 150.0); // Varying prices

            let _order_mock = server.mock(|when, then| {
                when.method(GET)
                    .path(format!(
                        "/trader/v1/accounts/ABC123DEF456/orders/{order_id}"
                    ))
                    .header("authorization", "Bearer test_access_token");
                then.status(200)
                    .header("content-type", "application/json")
                    .json_body(json!({
                        "orderId": i as u64 + 1_004_055_538_000,
                        "status": "FILLED",
                        "filledQuantity": (i as f64).mul_add(10.0, 100.0),
                        "remainingQuantity": 0.0,
                        "enteredTime": "2023-10-15T10:25:00Z",
                        "closeTime": "2023-10-15T10:30:00Z",
                        "orderActivityCollection": [{
                            "activityType": "EXECUTION",
                            "executionLegs": [{
                                "executionId": format!("EXEC{i:04}"),
                                "quantity": (i as f64).mul_add(10.0, 100.0),
                                "price": price,
                                "time": "2023-10-15T10:30:00Z"
                            }]
                        }]
                    }));
            });
        }

        // Configure poller for performance testing
        let config = OrderPollerConfig {
            polling_interval: std::time::Duration::from_millis(100), // Fast polling
            max_jitter: std::time::Duration::from_millis(10),
        };

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let poller = OrderStatusPoller::new(config, env, pool.clone(), shutdown_rx, Schwab);

        // Measure performance of concurrent polling
        let start_time = Instant::now();

        // Poll all executions sequentially (but time the batch)
        let mut results = Vec::new();
        for execution_id in execution_ids {
            let execution = find_execution_by_id(&pool, execution_id)
                .await
                .unwrap()
                .unwrap();
            let result = poller.poll_execution_status(&execution).await;
            results.push(result);
        }
        let elapsed = start_time.elapsed();

        println!(
            "Polled {num_orders} orders in {elapsed:?} ({:.2} orders/sec)",
            num_orders as f64 / elapsed.as_secs_f64()
        );

        // Verify all polls succeeded
        for (i, result) in results.iter().enumerate() {
            assert!(result.is_ok(), "Poll {i} returned error: {result:?}");
        }

        // Verify all executions were updated to FILLED
        let filled_executions = crate::schwab::execution::find_executions_by_symbol_and_status(
            &pool,
            "", // Empty string finds all symbols
            TradeStatus::Filled,
        )
        .await
        .unwrap();

        assert_eq!(filled_executions.len(), num_orders);

        // Performance assertions - use generous timeout to account for CI/instrumentation overhead
        assert!(
            elapsed.as_secs() < 30,
            "High volume polling took too long: {elapsed:?}"
        );
        assert!(
            (elapsed.as_secs_f64() / (num_orders as f64)) < 0.6,
            "Average time per order too high: {:.3}s",
            elapsed.as_secs_f64() / (num_orders as f64)
        );

        // Verify mocks were called appropriately
        account_mock.assert_hits(num_orders); // Called once per order status check

        // Trigger shutdown for clean test completion
        shutdown_tx.send(true).unwrap();
    }

    async fn setup_failed_order_test() -> (MockServer, SqlitePool, SchwabAuthEnv, i64) {
        let server = MockServer::start();
        let pool = setup_test_db().await;

        let env = SchwabAuthEnv {
            app_key: "test_key".to_string(),
            app_secret: "test_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: server.base_url(),
            account_index: 0,
        };

        let tokens = crate::schwab::SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: chrono::Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: chrono::Utc::now(),
        };
        tokens.store(&pool).await.unwrap();

        let execution = SchwabExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 100,
            direction: Direction::Buy,
            state: TradeState::Submitted {
                order_id: "FAILED_ORDER_123".to_string(),
            },
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let execution_id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();

        let calculator = crate::onchain::position_calculator::PositionCalculator::new();
        crate::onchain::accumulator::save_within_transaction(
            &mut sql_tx,
            &"TSLA".parse().unwrap(),
            &calculator,
            Some(execution_id),
        )
        .await
        .unwrap();

        crate::lock::try_acquire_execution_lease(&mut sql_tx, &"TSLA".parse().unwrap())
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();

        (server, pool, env, execution_id)
    }

    fn setup_failed_order_mocks(server: &MockServer) -> (Mock, Mock) {
        let account_mock = server.mock(|when, then| {
            when.method(GET).path("/trader/v1/accounts/accountNumbers");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!([{
                    "accountNumber": "123456789",
                    "hashValue": "ABC123DEF456"
                }]));
        });

        let order_status_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/trader/v1/accounts/ABC123DEF456/orders/FAILED_ORDER_123")
                .header("authorization", "Bearer test_access_token");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "orderId": 9_999_999_999_i64,
                    "status": "CANCELED",
                    "filledQuantity": 0.0,
                    "remainingQuantity": 100.0,
                    "enteredTime": "2023-10-15T10:25:00Z",
                    "closeTime": "2023-10-15T10:26:00Z"
                }));
        });

        (account_mock, order_status_mock)
    }

    async fn verify_failed_order_cleanup(pool: &SqlitePool, execution_id: i64) {
        let final_execution = find_execution_by_id(pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&final_execution.state, TradeState::Failed { .. }));

        let row = sqlx::query!(
            "SELECT pending_execution_id FROM trade_accumulators WHERE symbol = ?1",
            "TSLA"
        )
        .fetch_optional(pool)
        .await
        .unwrap();

        if let Some(row) = row {
            assert_eq!(
                row.pending_execution_id, None,
                "pending_execution_id should be cleared after order fails"
            );
        }

        let lock_count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM symbol_locks WHERE symbol = ?1",
            "TSLA"
        )
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(
            lock_count, 0,
            "Symbol lock should be cleared after order fails"
        );
    }

    #[tokio::test]
    async fn test_handle_failed_order_clears_pending_execution_id() {
        let (server, pool, env, execution_id) = setup_failed_order_test().await;
        let (account_mock, order_status_mock) = setup_failed_order_mocks(&server);

        // Verify pending_execution_id is set before the test
        let row = sqlx::query!(
            "SELECT pending_execution_id FROM trade_accumulators WHERE symbol = ?1",
            "TSLA"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.pending_execution_id, Some(execution_id));

        let config = OrderPollerConfig::default();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let poller = OrderStatusPoller::new(config, env, pool.clone(), shutdown_rx, Schwab);

        let execution = find_execution_by_id(&pool, execution_id)
            .await
            .unwrap()
            .unwrap();
        let poll_result = poller.poll_execution_status(&execution).await;
        assert!(poll_result.is_ok());

        verify_failed_order_cleanup(&pool, execution_id).await;

        account_mock.assert();
        order_status_mock.assert();
        shutdown_tx.send(true).unwrap();
    }
}
