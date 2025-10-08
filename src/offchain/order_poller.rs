use rand::Rng;
use sqlx::SqlitePool;
use std::time::Duration;
use tokio::time::{Interval, interval};
use tracing::{debug, error, info};

use super::execution::{
    OffchainExecution, find_execution_by_id, find_executions_by_symbol_and_status,
    update_execution_status_within_transaction,
};
use crate::error::OrderPollingError;
use crate::lock::{clear_execution_lease, clear_pending_execution_id};
use st0x_broker::{Broker, OrderState, OrderStatus, PersistenceError};

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

pub struct OrderStatusPoller<B: Broker> {
    config: OrderPollerConfig,
    pool: SqlitePool,
    interval: Interval,
    broker: B,
}

impl<B: Broker> OrderStatusPoller<B> {
    pub fn new(config: OrderPollerConfig, pool: SqlitePool, broker: B) -> Self {
        let interval = interval(config.polling_interval);

        Self {
            config,
            pool,
            interval,
            broker,
        }
    }

    pub async fn run(mut self) -> Result<(), OrderPollingError> {
        info!(
            "Starting order status poller with interval: {:?}",
            self.config.polling_interval
        );

        loop {
            self.interval.tick().await;

            if let Err(e) = self.poll_pending_orders().await {
                error!("Polling cycle failed: {e}");
            }
        }
    }

    async fn poll_pending_orders(&self) -> Result<(), OrderPollingError> {
        debug!("Starting polling cycle for submitted orders");

        let submitted_executions =
            find_executions_by_symbol_and_status(&self.pool, "", OrderStatus::Submitted)
                .await
                .map_err(|e| {
                    error!("Failed to query pending executions: {e}");
                    OrderPollingError::from(e)
                })?;

        if submitted_executions.is_empty() {
            debug!("No submitted orders to poll");
            return Ok(());
        }

        info!("Polling {} submitted orders", submitted_executions.len());

        for execution in submitted_executions {
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
        execution: &OffchainExecution,
    ) -> Result<(), OrderPollingError> {
        let Some(execution_id) = execution.id else {
            error!("Execution missing ID: {execution:?}");
            return Ok(());
        };

        let order_id = match &execution.state {
            OrderState::Pending => {
                debug!("Execution {execution_id} is PENDING but no order_id yet");
                return Ok(());
            }
            OrderState::Submitted { order_id } | OrderState::Filled { order_id, .. } => {
                order_id.clone()
            }
            OrderState::Failed { .. } => {
                debug!("Execution {execution_id} already failed, skipping poll");
                return Ok(());
            }
        };

        let parsed_order_id = self
            .broker
            .parse_order_id(&order_id)
            .map_err(|e| OrderPollingError::Broker(Box::new(e)))?;

        let order_state = self
            .broker
            .get_order_status(&parsed_order_id)
            .await
            .map_err(|e| OrderPollingError::Broker(Box::new(e)))?;

        match &order_state {
            OrderState::Filled { .. } => {
                self.handle_filled_order(execution_id, &order_state).await?;
            }
            OrderState::Failed { .. } => {
                self.handle_failed_order(execution_id, &order_state).await?;
            }
            OrderState::Pending | OrderState::Submitted { .. } => {
                debug!(
                    "Order {order_id} (execution {execution_id}) still pending with state: {:?}",
                    order_state
                );
            }
        }

        Ok(())
    }

    async fn handle_filled_order(
        &self,
        execution_id: i64,
        order_state: &OrderState,
    ) -> Result<(), OrderPollingError> {
        let new_status = order_state.clone();

        let mut tx = self.pool.begin().await?;

        let execution = find_execution_by_id(&self.pool, execution_id)
            .await
            .map_err(|e| {
                error!("Failed to find execution {execution_id}: {e}");
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Database query failed".to_string(),
                ))
            })?
            .ok_or_else(|| {
                error!("Execution {execution_id} not found in database");
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Execution not found".to_string(),
                ))
            })?;

        update_execution_status_within_transaction(&mut tx, execution_id, &new_status).await?;

        clear_pending_execution_id(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Invalid symbol in execution".to_string(),
                ))
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear pending execution ID for symbol {}: {e}",
                execution.symbol
            );
            OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                "Failed to clear pending execution ID".to_string(),
            ))
        })?;

        clear_execution_lease(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Invalid symbol in execution".to_string(),
                ))
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear execution lease for symbol {}: {e}",
                execution.symbol
            );
            OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                "Failed to clear execution lease".to_string(),
            ))
        })?;

        tx.commit().await?;

        if let OrderState::Filled { price_cents, .. } = order_state {
            info!(
                "Updated execution {execution_id} to FILLED with price: {} cents and cleared locks for symbol: {}",
                price_cents, execution.symbol
            );
        } else {
            info!(
                "Updated execution {execution_id} to FILLED and cleared locks for symbol: {}",
                execution.symbol
            );
        }

        Ok(())
    }

    async fn handle_failed_order(
        &self,
        execution_id: i64,
        order_state: &OrderState,
    ) -> Result<(), OrderPollingError> {
        let new_status = order_state.clone();

        let mut tx = self.pool.begin().await?;

        let execution = find_execution_by_id(&self.pool, execution_id)
            .await
            .map_err(|e| {
                error!("Failed to find execution {execution_id}: {e}");
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Database query failed".to_string(),
                ))
            })?
            .ok_or_else(|| {
                error!("Execution {execution_id} not found in database");
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Execution not found".to_string(),
                ))
            })?;

        update_execution_status_within_transaction(&mut tx, execution_id, &new_status).await?;

        clear_pending_execution_id(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Invalid symbol in execution".to_string(),
                ))
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear pending execution ID for symbol {}: {e}",
                execution.symbol
            );
            OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                "Failed to clear pending execution ID".to_string(),
            ))
        })?;

        clear_execution_lease(
            &mut tx,
            &execution.symbol.parse().map_err(|e| {
                error!("Failed to parse symbol {}: {e}", execution.symbol);
                OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                    "Invalid symbol in execution".to_string(),
                ))
            })?,
        )
        .await
        .map_err(|e| {
            error!(
                "Failed to clear execution lease for symbol {}: {e}",
                execution.symbol
            );
            OrderPollingError::Persistence(PersistenceError::InvalidTradeStatus(
                "Failed to clear execution lease".to_string(),
            ))
        })?;

        tx.commit().await?;

        info!(
            "Updated execution {execution_id} to FAILED and cleared locks for symbol: {}",
            execution.symbol
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
