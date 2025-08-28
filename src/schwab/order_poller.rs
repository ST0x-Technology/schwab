use chrono::Utc;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Interval, interval};
use tracing::{debug, error, info};

use super::execution::{
    find_execution_by_id, find_executions_by_symbol_and_status,
    update_execution_status_within_transaction,
};
use super::order::Order;
use super::{SchwabAuthEnv, SchwabError, TradeState, TradeStatus};

#[derive(Debug, Clone)]
pub(crate) struct OrderPollerConfig {
    pub(crate) polling_interval: Duration,
    pub(crate) max_jitter: Duration,
}

impl Default for OrderPollerConfig {
    fn default() -> Self {
        Self {
            polling_interval: Duration::from_secs(15),
            max_jitter: Duration::from_secs(5),
        }
    }
}

pub(crate) struct OrderStatusPoller {
    config: OrderPollerConfig,
    env: SchwabAuthEnv,
    pool: SqlitePool,
    interval: Interval,
    shutdown_rx: watch::Receiver<bool>,
}

impl OrderStatusPoller {
    pub(crate) fn new(
        config: OrderPollerConfig,
        env: SchwabAuthEnv,
        pool: SqlitePool,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Self {
        let interval = interval(config.polling_interval);

        Self {
            config,
            env,
            pool,
            interval,
            shutdown_rx,
        }
    }

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
            find_executions_by_symbol_and_status(&self.pool, "", TradeStatus::Submitted)
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

            if let Err(e) = self.poll_execution_status(execution_id).await {
                error!("Failed to poll execution {execution_id}: {e}");
            }

            self.add_jittered_delay(execution_id).await;
        }

        debug!("Completed polling cycle");
        Ok(())
    }

    async fn poll_execution_status(&self, execution_id: i64) -> Result<(), SchwabError> {
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

        let order_status = Order::get_order_status(&order_id, &self.env, &self.pool).await?;

        if order_status.is_filled() {
            self.handle_filled_order(execution_id, &order_status)
                .await?;
        } else if order_status.is_terminal_failure() {
            self.handle_failed_order(execution_id, &order_status)
                .await?;
        } else {
            debug!(
                "Order {order_id} (execution {execution_id}) still pending with status: {:?}",
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

        let new_state = TradeState::Filled {
            executed_at: Utc::now(),
            order_id: order_status
                .order_id
                .clone()
                .unwrap_or_else(|| format!("UNKNOWN_{execution_id}")),
            price_cents,
        };

        self.update_execution_status(execution_id, new_state)
            .await?;

        info!(
            "Updated execution {execution_id} to FILLED with price: {} cents",
            price_cents
        );

        Ok(())
    }

    async fn handle_failed_order(
        &self,
        execution_id: i64,
        order_status: &super::order_status::OrderStatusResponse,
    ) -> Result<(), SchwabError> {
        let new_state = TradeState::Failed {
            failed_at: Utc::now(),
            error_reason: Some(format!("Order status: {:?}", order_status.status)),
        };

        self.update_execution_status(execution_id, new_state)
            .await?;

        info!(
            "Updated execution {execution_id} to FAILED due to order status: {:?}",
            order_status.status
        );

        Ok(())
    }

    async fn update_execution_status(
        &self,
        execution_id: i64,
        new_state: TradeState,
    ) -> Result<(), SchwabError> {
        let mut tx = self.pool.begin().await?;
        update_execution_status_within_transaction(&mut tx, execution_id, new_state).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_jittered_delay(&self, execution_id: i64) {
        if self.config.max_jitter > Duration::ZERO {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let jitter_millis =
                (execution_id as u64 * 17 + 42) % self.config.max_jitter.as_millis() as u64;
            let jitter = Duration::from_millis(jitter_millis);
            tokio::time::sleep(jitter).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schwab::Direction;
    use crate::schwab::execution::SchwabExecution;
    use crate::test_utils::setup_test_db;
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

        let poller = OrderStatusPoller::new(config.clone(), env, pool, shutdown_rx);
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

        let poller = OrderStatusPoller::new(config, env, pool, shutdown_rx);

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

        let poller = OrderStatusPoller::new(config, env, pool, shutdown_rx);

        let result = poller.poll_execution_status(execution_id).await;
        assert!(result.is_ok());
    }
}
