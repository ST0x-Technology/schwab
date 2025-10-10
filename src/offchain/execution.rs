use sqlx::SqlitePool;

use crate::error::OnChainError;
use st0x_broker::PersistenceError;
use st0x_broker::{Direction, SupportedBroker};
use st0x_broker::{OrderState, OrderStatus};

#[derive(sqlx::FromRow)]
struct ExecutionRow {
    id: i64,
    symbol: String,
    shares: i64,
    direction: String,
    broker: String,
    order_id: Option<String>,
    price_cents: Option<i64>,
    status: String,
    executed_at: Option<chrono::NaiveDateTime>,
}

/// Converts database row data to an OffchainExecution instance.
/// Centralizes the conversion logic and casting operations.
fn row_to_execution(
    ExecutionRow {
        id,
        symbol,
        shares,
        direction,
        broker,
        order_id,
        price_cents,
        status,
        executed_at,
    }: ExecutionRow,
) -> Result<OffchainExecution, OnChainError> {
    let parsed_direction = direction.parse()?;
    let parsed_broker = match broker.as_str() {
        "schwab" => SupportedBroker::Schwab,
        "alpaca" => SupportedBroker::Alpaca,
        "dry_run" => SupportedBroker::DryRun,
        _ => {
            return Err(OnChainError::Persistence(
                PersistenceError::InvalidTradeStatus(format!("Unknown broker type: {broker}")),
            ));
        }
    };
    let status_enum = match status.as_str() {
        "PENDING" => OrderStatus::Pending,
        "SUBMITTED" => OrderStatus::Submitted,
        "FILLED" => OrderStatus::Filled,
        "FAILED" => OrderStatus::Failed,
        _ => {
            return Err(OnChainError::Persistence(
                PersistenceError::InvalidTradeStatus(format!("Invalid order status: {status}")),
            ));
        }
    };
    let parsed_state = OrderState::from_db_row(status_enum, order_id, price_cents, executed_at)
        .map_err(|e| {
            OnChainError::Persistence(PersistenceError::InvalidTradeStatus(e.to_string()))
        })?;

    Ok(OffchainExecution {
        id: Some(id),
        symbol,
        shares: shares.try_into().map_err(|_| {
            OnChainError::Persistence(PersistenceError::InvalidShareQuantity(shares))
        })?,
        direction: parsed_direction,
        broker: parsed_broker,
        state: parsed_state,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffchainExecution {
    pub id: Option<i64>,
    pub symbol: String,
    pub shares: u64,
    pub direction: Direction,
    pub broker: SupportedBroker,
    pub state: OrderState,
}

pub async fn update_execution_status_within_transaction(
    sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    execution_id: i64,
    new_state: &OrderState,
) -> Result<(), PersistenceError> {
    let status_str = new_state.status().as_str();
    let db_fields = new_state.to_db_fields()?;

    sqlx::query!(
        "
        UPDATE offchain_trades
        SET status = ?1, order_id = ?2, price_cents = ?3, executed_at = ?4
        WHERE id = ?5
        ",
        status_str,
        db_fields.order_id,
        db_fields.price_cents,
        db_fields.executed_at,
        execution_id
    )
    .execute(&mut **sql_tx)
    .await?;

    Ok(())
}

pub trait HasTradeStatus {
    fn status_str(&self) -> &str;
}

impl HasTradeStatus for &str {
    fn status_str(&self) -> &str {
        self
    }
}

impl HasTradeStatus for OrderStatus {
    fn status_str(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
pub(crate) async fn find_executions_by_symbol_and_status<S: HasTradeStatus>(
    pool: &SqlitePool,
    symbol: &str,
    status: S,
) -> Result<Vec<OffchainExecution>, OnChainError> {
    find_executions_by_symbol_status_and_broker(pool, symbol, status, None).await
}

pub async fn find_executions_by_symbol_status_and_broker<S: HasTradeStatus>(
    pool: &SqlitePool,
    symbol: &str,
    status: S,
    broker: Option<SupportedBroker>,
) -> Result<Vec<OffchainExecution>, OnChainError> {
    let status_str = status.status_str();

    let (query, params): (String, Vec<String>) = if symbol.is_empty() {
        broker.map_or_else(
            || {
                (
                    "SELECT * FROM offchain_trades WHERE status = ?1 ORDER BY id ASC".to_string(),
                    vec![status_str.to_string()],
                )
            },
            |broker| {
                (
                    "SELECT * FROM offchain_trades WHERE status = ?1 AND broker = ?2 ORDER BY id ASC"
                        .to_string(),
                    vec![status_str.to_string(), broker.to_string()],
                )
            },
        )
    } else {
        broker.map_or_else(
            || {
                (
                    "SELECT * FROM offchain_trades WHERE symbol = ?1 AND status = ?2 ORDER BY id ASC"
                        .to_string(),
                    vec![symbol.to_string(), status_str.to_string()],
                )
            },
            |broker| {
                (
                    "SELECT * FROM offchain_trades WHERE symbol = ?1 AND status = ?2 AND broker = ?3 ORDER BY id ASC"
                        .to_string(),
                    vec![
                        symbol.to_string(),
                        status_str.to_string(),
                        broker.to_string(),
                    ],
                )
            },
        )
    };

    let mut query_builder = sqlx::query_as::<_, ExecutionRow>(&query);
    for param in params {
        query_builder = query_builder.bind(param);
    }

    let rows = query_builder.fetch_all(pool).await?;

    rows.into_iter()
        .map(row_to_execution)
        .collect::<Result<Vec<_>, _>>()
}

pub async fn find_execution_by_id(
    pool: &SqlitePool,
    execution_id: i64,
) -> Result<Option<OffchainExecution>, OnChainError> {
    let row = sqlx::query!("SELECT * FROM offchain_trades WHERE id = ?1", execution_id)
        .fetch_optional(pool)
        .await?;

    if let Some(row) = row {
        row_to_execution(ExecutionRow {
            id: row.id,
            symbol: row.symbol,
            shares: row.shares,
            direction: row.direction,
            broker: row.broker,
            order_id: row.order_id,
            price_cents: row.price_cents,
            status: row.status,
            executed_at: row.executed_at,
        })
        .map(Some)
    } else {
        Ok(None)
    }
}

impl OffchainExecution {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, st0x_broker::PersistenceError> {
        let shares_i64 = i64::try_from(self.shares).map_err(|_| {
            PersistenceError::InvalidShareQuantity({
                #[allow(clippy::cast_possible_wrap)]
                (self.shares as i64)
            })
        })?;
        let direction_str = self.direction.as_str();
        let broker_str = self.broker.to_string();
        let status_str = self.state.status().as_str();
        let db_fields = self.state.to_db_fields()?;

        let result = sqlx::query!(
            r#"
            INSERT INTO offchain_trades (
                symbol,
                shares,
                direction,
                broker,
                order_id,
                price_cents,
                status,
                executed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            self.symbol,
            shares_i64,
            direction_str,
            broker_str,
            db_fields.order_id,
            db_fields.price_cents,
            status_str,
            db_fields.executed_at
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{OffchainExecutionBuilder, setup_test_db};
    use chrono::Utc;
    use st0x_broker::OrderState;

    #[tokio::test]
    async fn test_offchain_execution_save_and_find() {
        let pool = setup_test_db().await;

        let execution = OffchainExecutionBuilder::new().build();

        let mut sql_tx = pool.begin().await.unwrap();
        let id = execution
            .save_within_transaction(&mut sql_tx)
            .await
            .unwrap();
        sql_tx.commit().await.unwrap();
        assert!(id > 0);

        let count = sqlx::query!("SELECT COUNT(*) as count FROM offchain_trades")
            .fetch_one(&pool)
            .await
            .unwrap()
            .count;
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_find_by_symbol_and_status() {
        let pool = setup_test_db().await;

        let execution1 = OffchainExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 50,
            direction: Direction::Buy,
            broker: SupportedBroker::Schwab,
            state: OrderState::Pending,
        };

        let execution2 = OffchainExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 25,
            direction: Direction::Sell,
            broker: SupportedBroker::Schwab,
            state: OrderState::Filled {
                executed_at: Utc::now(),
                order_id: "1004055538123".to_string(),
                price_cents: 15025,
            },
        };

        let execution3 = OffchainExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 10,
            direction: Direction::Buy,
            broker: SupportedBroker::Schwab,
            state: OrderState::Pending,
        };

        let mut sql_tx1 = pool.begin().await.unwrap();
        execution1
            .save_within_transaction(&mut sql_tx1)
            .await
            .unwrap();
        sql_tx1.commit().await.unwrap();

        let mut sql_tx2 = pool.begin().await.unwrap();
        execution2
            .save_within_transaction(&mut sql_tx2)
            .await
            .unwrap();
        sql_tx2.commit().await.unwrap();

        let mut sql_tx3 = pool.begin().await.unwrap();
        execution3
            .save_within_transaction(&mut sql_tx3)
            .await
            .unwrap();
        sql_tx3.commit().await.unwrap();

        let pending_aapl = find_executions_by_symbol_and_status(&pool, "AAPL", "PENDING")
            .await
            .unwrap();

        assert_eq!(pending_aapl.len(), 1);
        assert_eq!(pending_aapl[0].shares, 50);
        assert_eq!(pending_aapl[0].direction, Direction::Buy);

        let completed_aapl = find_executions_by_symbol_and_status(&pool, "AAPL", "FILLED")
            .await
            .unwrap();

        assert_eq!(completed_aapl.len(), 1);
        assert_eq!(completed_aapl[0].shares, 25);
        assert_eq!(completed_aapl[0].direction, Direction::Sell);
        assert!(matches!(
            &completed_aapl[0].state,
            OrderState::Filled { order_id, price_cents, .. }
            if order_id == "1004055538123" && *price_cents == 15025
        ));
    }

    #[tokio::test]
    async fn test_database_tracks_different_brokers() {
        let pool = setup_test_db().await;

        let schwab_execution = OffchainExecution {
            id: None,
            symbol: "AAPL".to_string(),
            shares: 100,
            direction: Direction::Buy,
            broker: SupportedBroker::Schwab,
            state: OrderState::Pending,
        };

        let alpaca_execution = OffchainExecution {
            id: None,
            symbol: "TSLA".to_string(),
            shares: 50,
            direction: Direction::Sell,
            broker: SupportedBroker::Alpaca,
            state: OrderState::Pending,
        };

        let dry_run_execution = OffchainExecution {
            id: None,
            symbol: "MSFT".to_string(),
            shares: 25,
            direction: Direction::Buy,
            broker: SupportedBroker::DryRun,
            state: OrderState::Pending,
        };

        let mut sql_tx1 = pool.begin().await.unwrap();
        let schwab_id = schwab_execution
            .save_within_transaction(&mut sql_tx1)
            .await
            .unwrap();
        sql_tx1.commit().await.unwrap();

        let mut sql_tx2 = pool.begin().await.unwrap();
        let alpaca_id = alpaca_execution
            .save_within_transaction(&mut sql_tx2)
            .await
            .unwrap();
        sql_tx2.commit().await.unwrap();

        let mut sql_tx3 = pool.begin().await.unwrap();
        let dry_run_id = dry_run_execution
            .save_within_transaction(&mut sql_tx3)
            .await
            .unwrap();
        sql_tx3.commit().await.unwrap();

        let schwab_retrieved = find_execution_by_id(&pool, schwab_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(schwab_retrieved.broker, SupportedBroker::Schwab);
        assert_eq!(schwab_retrieved.symbol, "AAPL");
        assert_eq!(schwab_retrieved.shares, 100);

        let alpaca_retrieved = find_execution_by_id(&pool, alpaca_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(alpaca_retrieved.broker, SupportedBroker::Alpaca);
        assert_eq!(alpaca_retrieved.symbol, "TSLA");
        assert_eq!(alpaca_retrieved.shares, 50);

        let dry_run_retrieved = find_execution_by_id(&pool, dry_run_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(dry_run_retrieved.broker, SupportedBroker::DryRun);
        assert_eq!(dry_run_retrieved.symbol, "MSFT");
        assert_eq!(dry_run_retrieved.shares, 25);

        let all_pending = find_executions_by_symbol_status_and_broker(&pool, "", "PENDING", None)
            .await
            .unwrap();
        assert_eq!(all_pending.len(), 3);

        let schwab_only = find_executions_by_symbol_status_and_broker(
            &pool,
            "",
            "PENDING",
            Some(SupportedBroker::Schwab),
        )
        .await
        .unwrap();
        assert_eq!(schwab_only.len(), 1);
        assert_eq!(schwab_only[0].broker, SupportedBroker::Schwab);

        let alpaca_only = find_executions_by_symbol_status_and_broker(
            &pool,
            "",
            "PENDING",
            Some(SupportedBroker::Alpaca),
        )
        .await
        .unwrap();
        assert_eq!(alpaca_only.len(), 1);
        assert_eq!(alpaca_only[0].broker, SupportedBroker::Alpaca);

        let dry_run_only = find_executions_by_symbol_status_and_broker(
            &pool,
            "",
            "PENDING",
            Some(SupportedBroker::DryRun),
        )
        .await
        .unwrap();
        assert_eq!(dry_run_only.len(), 1);
        assert_eq!(dry_run_only[0].broker, SupportedBroker::DryRun);
    }
}
