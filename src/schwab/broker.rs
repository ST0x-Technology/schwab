use backon::{ExponentialBuilder, Retryable};
use opentelemetry::KeyValue;
use reqwest::header::{self, HeaderMap, HeaderValue};
use sqlx::SqlitePool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;
use tracing::{error, warn};

/// Type alias for a dynamic broker trait object wrapped in Arc
pub(crate) type DynBroker = Arc<dyn Broker + Send + Sync>;

use super::{
    SchwabAuthEnv, SchwabError, SchwabInstruction,
    execution::SchwabExecution,
    order::{Instruction, Order, handle_execution_failure, handle_execution_success},
    order_status::{ExecutionLeg, OrderActivity, OrderStatus, OrderStatusResponse},
    tokens::SchwabTokens,
};
use crate::env::Env;
use crate::metrics;

/// Trait for order execution abstraction supporting both real and mock brokers
pub(crate) trait Broker: Send + Sync + std::fmt::Debug {
    fn execute_order<'a>(
        &'a self,
        env: &'a Env,
        pool: &'a SqlitePool,
        execution: SchwabExecution,
        metrics: Arc<Option<metrics::Metrics>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SchwabError>> + Send + 'a>>;

    fn get_order_status<'a>(
        &'a self,
        order_id: &'a str,
        env: &'a SchwabAuthEnv,
        pool: &'a SqlitePool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<OrderStatusResponse, SchwabError>> + Send + 'a>,
    >;
}

/// Real Schwab broker implementation
#[derive(Debug, Clone)]
pub(crate) struct Schwab;

impl Broker for Schwab {
    /// Execute a Schwab order using the unified system.
    /// Takes a SchwabExecution and places the corresponding order via Schwab API.
    fn execute_order<'a>(
        &'a self,
        env: &'a Env,
        pool: &'a SqlitePool,
        execution: SchwabExecution,
        metrics: Arc<Option<metrics::Metrics>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SchwabError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Record start time for duration tracking
            let start_time = Instant::now();

            // Prepare metrics labels
            let symbol = execution.symbol.clone();
            let direction = match execution.direction {
                SchwabInstruction::Buy => "buy",
                SchwabInstruction::Sell => "sell",
            };

            // Increment schwab_orders_executed with "pending" status
            if let Some(ref m) = *metrics {
                m.schwab_orders_executed.add(
                    1,
                    &[
                        KeyValue::new("status", "pending"),
                        KeyValue::new("symbol", symbol.clone()),
                        KeyValue::new("direction", direction),
                    ],
                );
            }

            let schwab_instruction = match execution.direction {
                SchwabInstruction::Buy => Instruction::Buy,
                SchwabInstruction::Sell => Instruction::Sell,
            };

            let order = Order::new(
                execution.symbol.clone(),
                schwab_instruction,
                execution.shares,
            );

            let result = (|| async { order.place(&env.schwab_auth, pool).await })
                .retry(&ExponentialBuilder::default().with_max_times(3))
                .await;

            let execution_id = execution.id.ok_or_else(|| {
                error!("SchwabExecution missing ID when executing: {execution:?}");
                SchwabError::RequestFailed {
                    action: "execute order".to_string(),
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Execution missing database ID".to_string(),
                }
            })?;

            match result {
                Ok(response) => {
                    handle_execution_success(pool, execution_id, response.order_id).await?;

                    // Record success metrics with duration
                    let duration_ms = start_time.elapsed().as_millis() as f64;
                    if let Some(ref m) = *metrics {
                        m.schwab_orders_executed.add(
                            1,
                            &[
                                KeyValue::new("status", "success"),
                                KeyValue::new("symbol", symbol.clone()),
                                KeyValue::new("direction", direction),
                            ],
                        );
                        m.trade_execution_duration_ms.record(
                            duration_ms,
                            &[KeyValue::new("operation", "schwab_order_execution")],
                        );
                    }
                }
                Err(e) => {
                    handle_execution_failure(pool, execution_id, e).await?;

                    // Record failure metrics
                    if let Some(ref m) = *metrics {
                        m.schwab_orders_executed.add(
                            1,
                            &[
                                KeyValue::new("status", "failed"),
                                KeyValue::new("symbol", symbol),
                                KeyValue::new("direction", direction),
                            ],
                        );
                    }
                }
            }

            Ok(())
        })
    }

    /// Get the status of a specific order from Schwab API.
    /// Returns the order status response containing fill information and execution details.
    fn get_order_status<'a>(
        &'a self,
        order_id: &'a str,
        env: &'a SchwabAuthEnv,
        pool: &'a SqlitePool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<OrderStatusResponse, SchwabError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let access_token = SchwabTokens::get_valid_access_token(pool, env).await?;
            let account_hash = env.get_account_hash(pool).await?;

            let headers = [
                (
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {access_token}"))?,
                ),
                (header::ACCEPT, HeaderValue::from_str("application/json")?),
            ]
            .into_iter()
            .collect::<HeaderMap>();

            let client = reqwest::Client::new();
            let response = (|| async {
                client
                    .get(format!(
                        "{}/trader/v1/accounts/{}/orders/{}",
                        env.base_url, account_hash, order_id
                    ))
                    .headers(headers.clone())
                    .send()
                    .await
            })
            .retry(ExponentialBuilder::default())
            .await?;

            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(SchwabError::RequestFailed {
                    action: "get order status".to_string(),
                    status,
                    body: format!("Order ID {order_id} not found"),
                });
            }

            if !response.status().is_success() {
                let error_body = response.text().await.unwrap_or_default();
                return Err(SchwabError::RequestFailed {
                    action: "get order status".to_string(),
                    status,
                    body: error_body,
                });
            }

            // Capture response text for debugging parse errors
            let response_text = response.text().await?;

            // Log successful response in debug mode to understand API structure
            tracing::debug!("Schwab order status response: {}", response_text);

            match serde_json::from_str::<OrderStatusResponse>(&response_text) {
                Ok(order_status) => Ok(order_status),
                Err(parse_error) => {
                    error!(
                        order_id = %order_id,
                        response_text = %response_text,
                        parse_error = %parse_error,
                        "Failed to parse Schwab order status response"
                    );
                    Err(SchwabError::InvalidConfiguration(format!(
                        "Failed to parse order status response: {parse_error}"
                    )))
                }
            }
        })
    }
}

/// Mock broker for dry-run mode
#[derive(Debug, Clone)]
pub(crate) struct LogBroker {
    order_counter: Arc<AtomicU64>,
}

impl LogBroker {
    pub(crate) fn new() -> Self {
        Self {
            order_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    fn generate_order_id(&self) -> String {
        let id = self.order_counter.fetch_add(1, Ordering::SeqCst);
        id.to_string()
    }
}

impl Broker for LogBroker {
    fn execute_order<'a>(
        &'a self,
        _env: &'a Env,
        pool: &'a SqlitePool,
        execution: SchwabExecution,
        metrics: Arc<Option<metrics::Metrics>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SchwabError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Record start time for duration tracking
            let start_time = Instant::now();

            // Prepare metrics labels
            let symbol = execution.symbol.clone();
            let direction = match execution.direction {
                SchwabInstruction::Buy => "buy",
                SchwabInstruction::Sell => "sell",
            };

            // Increment schwab_orders_executed with "pending" status
            if let Some(ref m) = *metrics {
                m.schwab_orders_executed.add(
                    1,
                    &[
                        KeyValue::new("status", "pending"),
                        KeyValue::new("symbol", symbol.clone()),
                        KeyValue::new("direction", direction),
                    ],
                );
            }

            let order_id = self.generate_order_id();

            warn!(
                "[DRY-RUN] Would execute order: {} {} shares of {} (execution_id: {:?})",
                execution.direction.as_str(),
                execution.shares,
                execution.symbol,
                execution.id
            );

            let execution_id = execution.id.ok_or_else(|| {
                error!("[DRY-RUN] SchwabExecution missing ID when executing: {execution:?}");
                SchwabError::RequestFailed {
                    action: "execute order".to_string(),
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Execution missing database ID".to_string(),
                }
            })?;

            warn!("[DRY-RUN] Generated mock order ID: {order_id}");

            // Simulate successful order placement by updating status to Submitted
            handle_execution_success(pool, execution_id, order_id).await?;

            // Record success metrics with duration
            let duration_ms = start_time.elapsed().as_millis() as f64;
            if let Some(ref m) = *metrics {
                m.schwab_orders_executed.add(
                    1,
                    &[
                        KeyValue::new("status", "success"),
                        KeyValue::new("symbol", symbol),
                        KeyValue::new("direction", direction),
                    ],
                );
                m.trade_execution_duration_ms.record(
                    duration_ms,
                    &[KeyValue::new("operation", "schwab_order_execution")],
                );
            }

            Ok(())
        })
    }

    fn get_order_status<'a>(
        &'a self,
        order_id: &'a str,
        _env: &'a SchwabAuthEnv,
        _pool: &'a SqlitePool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<OrderStatusResponse, SchwabError>> + Send + 'a>,
    > {
        Box::pin(async move {
            warn!("[DRY-RUN] Checking status for order: {order_id}");

            // Generate mock filled response with arbitrary price
            let mock_price = 100.50; // Arbitrary fill price
            let mock_quantity = 100.0; // Arbitrary quantity

            warn!("[DRY-RUN] Returning mock FILLED status with price: ${mock_price}");

            // Create a mock OrderStatusResponse that indicates the order is filled
            let response = OrderStatusResponse {
                order_id: Some(order_id.to_string()),
                status: Some(OrderStatus::Filled),
                filled_quantity: Some(mock_quantity),
                remaining_quantity: Some(0.0),
                entered_time: Some("2023-10-15T10:25:00Z".to_string()),
                close_time: Some("2023-10-15T10:30:00Z".to_string()),
                order_activity_collection: Some(vec![OrderActivity {
                    activity_type: Some("EXECUTION".to_string()),
                    execution_legs: Some(vec![ExecutionLeg {
                        quantity: mock_quantity,
                        price: mock_price,
                    }]),
                }]),
            };

            Ok(response)
        })
    }
}

// Implement Broker trait for Arc<dyn Broker>
impl Broker for DynBroker {
    fn execute_order<'a>(
        &'a self,
        env: &'a Env,
        pool: &'a SqlitePool,
        execution: SchwabExecution,
        metrics: Arc<Option<metrics::Metrics>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SchwabError>> + Send + 'a>>
    {
        (**self).execute_order(env, pool, execution, metrics)
    }

    fn get_order_status<'a>(
        &'a self,
        order_id: &'a str,
        env: &'a SchwabAuthEnv,
        pool: &'a SqlitePool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<OrderStatusResponse, SchwabError>> + Send + 'a>,
    > {
        (**self).get_order_status(order_id, env, pool)
    }
}

// Implement Broker trait for references to brokers
impl<B: Broker> Broker for &B {
    fn execute_order<'a>(
        &'a self,
        env: &'a Env,
        pool: &'a SqlitePool,
        execution: SchwabExecution,
        metrics: Arc<Option<metrics::Metrics>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SchwabError>> + Send + 'a>>
    {
        (**self).execute_order(env, pool, execution, metrics)
    }

    fn get_order_status<'a>(
        &'a self,
        order_id: &'a str,
        env: &'a SchwabAuthEnv,
        pool: &'a SqlitePool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<OrderStatusResponse, SchwabError>> + Send + 'a>,
    > {
        (**self).get_order_status(order_id, env, pool)
    }
}
