use chrono::{DateTime, Utc};
use clap::Parser;
use pnl::{FifoInventory, PnlError, PnlResult, TradeType};
use rust_decimal::Decimal;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tracing::{error, info};

use crate::schwab::Direction;
use crate::symbol::Symbol;

mod pnl;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct ReporterEnv {
    #[clap(long, env, default_value = "sqlite:./data/schwab.db")]
    database_url: String,
    #[clap(long, env, default_value = "30")]
    reporter_processing_interval_secs: u64,
    #[clap(long, env, default_value = "info")]
    log_level: crate::env::LogLevel,
}

impl crate::env::HasSqlite for ReporterEnv {
    async fn get_sqlite_pool(&self) -> Result<SqlitePool, sqlx::Error> {
        let pool = SqlitePool::connect(&self.database_url).await?;

        // SQLite Concurrency Configuration:
        //
        // WAL Mode: Allows concurrent readers but only ONE writer at a time across
        // all processes. When both main bot and reporter try to write simultaneously,
        // one will block until the other completes. This is a fundamental SQLite
        // limitation.
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&pool)
            .await?;

        // Busy Timeout: 10 seconds - when a write is blocked by another process,
        // SQLite will wait up to 10 seconds before failing with "database is locked".
        // This prevents immediate failures when main bot and reporter write concurrently.
        //
        // CRITICAL: Reporter must keep transactions SHORT (single INSERT per trade)
        // to avoid blocking mission-critical main bot operations.
        //
        // Future: This limitation will be eliminated when migrating to Kafka +
        // Elasticsearch with CQRS pattern for separate read/write paths.
        sqlx::query("PRAGMA busy_timeout = 10000")
            .execute(&pool)
            .await?;

        Ok(pool)
    }
}

impl ReporterEnv {
    pub fn log_level(&self) -> &crate::env::LogLevel {
        &self.log_level
    }

    fn processing_interval(&self) -> Duration {
        Duration::from_secs(self.reporter_processing_interval_secs)
    }
}

#[derive(Debug, Clone)]
struct Trade {
    id: i64,
    r#type: TradeType,
    symbol: Symbol,
    quantity: Decimal,
    price_per_share: Decimal,
    direction: Direction,
    timestamp: DateTime<Utc>,
}

impl Trade {
    fn from_onchain_row(
        id: i64,
        symbol: String,
        amount: f64,
        direction: String,
        price_usdc: f64,
        created_at: Option<chrono::NaiveDateTime>,
    ) -> anyhow::Result<Self> {
        let quantity = Decimal::from_str(&amount.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to convert amount to Decimal: {e}"))?;

        let price_per_share = Decimal::from_str(&price_usdc.to_string())
            .map_err(|e| anyhow::anyhow!("Failed to convert price_usdc to Decimal: {e}"))?;

        let direction = direction
            .parse()
            .map_err(|e: String| anyhow::anyhow!("Invalid direction: {e}"))?;

        let timestamp = created_at
            .ok_or_else(|| anyhow::anyhow!("created_at is NULL"))?
            .and_utc();

        Ok(Self {
            r#type: TradeType::Onchain,
            id,
            symbol: symbol.try_into()?,
            quantity,
            price_per_share,
            direction,
            timestamp,
        })
    }

    fn from_offchain_row(
        id: i64,
        symbol: String,
        shares: i64,
        direction: String,
        price_cents: Option<i64>,
        executed_at: Option<chrono::NaiveDateTime>,
    ) -> anyhow::Result<Self> {
        let executed_at =
            executed_at.ok_or_else(|| anyhow::anyhow!("FILLED execution missing executed_at"))?;

        let price_cents =
            price_cents.ok_or_else(|| anyhow::anyhow!("FILLED execution missing price_cents"))?;

        let quantity = Decimal::from(shares);

        let price_per_share = Decimal::from(price_cents)
            .checked_div(Decimal::from(100))
            .ok_or_else(|| anyhow::anyhow!("Division by 100 failed"))?;

        let direction = direction
            .parse()
            .map_err(|e: String| anyhow::anyhow!("Invalid direction: {e}"))?;

        Ok(Self {
            r#type: TradeType::Offchain,
            id,
            symbol: symbol.try_into()?,
            quantity,
            price_per_share,
            direction,
            timestamp: executed_at.and_utc(),
        })
    }

    fn to_db_values(&self, result: &PnlResult) -> anyhow::Result<DbMetricsRow> {
        let trade_type_str = match self.r#type {
            TradeType::Onchain => "ONCHAIN",
            TradeType::Offchain => "OFFCHAIN",
        };

        let direction_str = self.direction.as_str();

        let quantity_f64 = self
            .quantity
            .to_string()
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("Failed to convert quantity: {e}"))?;

        let price_per_share_f64 = self
            .price_per_share
            .to_string()
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("Failed to convert price_per_share: {e}"))?;

        let realized_pnl_f64 = result
            .realized_pnl
            .map(|p| {
                p.to_string()
                    .parse::<f64>()
                    .map_err(|e| anyhow::anyhow!("Failed to convert realized_pnl: {e}"))
            })
            .transpose()?;

        let cumulative_pnl_f64 = result
            .cumulative_pnl
            .to_string()
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("Failed to convert cumulative_pnl: {e}"))?;

        let net_position_after_f64 = result
            .net_position_after
            .to_string()
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("Failed to convert net_position_after: {e}"))?;

        Ok(DbMetricsRow {
            symbol: self.symbol.as_str().to_string(),
            timestamp: self.timestamp,
            trade_type: trade_type_str.to_string(),
            trade_id: self.id,
            trade_direction: direction_str.to_string(),
            quantity: quantity_f64,
            price_per_share: price_per_share_f64,
            realized_pnl: realized_pnl_f64,
            cumulative_pnl: cumulative_pnl_f64,
            net_position_after: net_position_after_f64,
        })
    }
}

struct DbMetricsRow {
    symbol: String,
    timestamp: DateTime<Utc>,
    trade_type: String,
    trade_id: i64,
    trade_direction: String,
    quantity: f64,
    price_per_share: f64,
    realized_pnl: Option<f64>,
    cumulative_pnl: f64,
    net_position_after: f64,
}

async fn load_checkpoint(pool: &SqlitePool) -> anyhow::Result<DateTime<Utc>> {
    let result = sqlx::query!("SELECT MAX(timestamp) as max_ts FROM metrics_pnl")
        .fetch_one(pool)
        .await?;

    result
        .max_ts
        .map(|ts| ts.and_utc())
        .ok_or_else(|| anyhow::anyhow!("No checkpoint found"))
        .or_else(|_| Ok(DateTime::UNIX_EPOCH))
}

async fn load_all_trades(pool: &SqlitePool) -> anyhow::Result<Vec<Trade>> {
    let onchain = sqlx::query!(
        "SELECT
            id,
            symbol,
            amount,
            direction,
            price_usdc,
            created_at
         FROM onchain_trades
         ORDER BY created_at, id"
    )
    .fetch_all(pool)
    .await?;

    let offchain = sqlx::query!(
        "SELECT
            id,
            symbol,
            shares,
            direction,
            price_cents,
            executed_at
         FROM schwab_executions
         WHERE status = 'FILLED'
         ORDER BY executed_at, id"
    )
    .fetch_all(pool)
    .await?;

    let onchain_trades = onchain
        .into_iter()
        .map(|row| {
            Trade::from_onchain_row(
                row.id,
                row.symbol,
                row.amount,
                row.direction,
                row.price_usdc,
                row.created_at,
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let offchain_trades = offchain
        .into_iter()
        .map(|row| {
            Trade::from_offchain_row(
                row.id,
                row.symbol,
                row.shares,
                row.direction,
                row.price_cents,
                row.executed_at,
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut trades = onchain_trades;
    trades.extend(offchain_trades);

    trades.sort_by_key(|t| (t.timestamp, t.r#type as u8, t.id));
    Ok(trades)
}

fn rebuild_fifo_state(
    trades: &[Trade],
    checkpoint: DateTime<Utc>,
) -> anyhow::Result<HashMap<Symbol, FifoInventory>> {
    trades
        .iter()
        .take_while(|t| t.timestamp <= checkpoint)
        .try_fold(HashMap::new(), |mut inventories, trade| {
            let inventory = inventories
                .entry(trade.symbol.clone())
                .or_insert_with(FifoInventory::new);

            inventory
                .process_trade(trade.quantity, trade.price_per_share, trade.direction)
                .map_err(|e| anyhow::anyhow!("FIFO processing error: {e}"))?;

            Ok(inventories)
        })
}

async fn persist_metrics_row(pool: &SqlitePool, row: &DbMetricsRow) -> anyhow::Result<()> {
    sqlx::query!(
        "INSERT INTO metrics_pnl (
            symbol,
            timestamp,
            trade_type,
            trade_id,
            trade_direction,
            quantity,
            price_per_share,
            realized_pnl,
            cumulative_pnl,
            net_position_after
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        row.symbol,
        row.timestamp,
        row.trade_type,
        row.trade_id,
        row.trade_direction,
        row.quantity,
        row.price_per_share,
        row.realized_pnl,
        row.cumulative_pnl,
        row.net_position_after,
    )
    .execute(pool)
    .await
    .map_err(|e| anyhow::anyhow!("Failed to insert into metrics_pnl: {e}"))?;

    Ok(())
}

async fn process_and_persist_trade(
    pool: &SqlitePool,
    inventories: &mut HashMap<Symbol, FifoInventory>,
    trade: &Trade,
) -> anyhow::Result<()> {
    let inventory = inventories
        .entry(trade.symbol.clone())
        .or_insert_with(FifoInventory::new);

    let result = inventory
        .process_trade(trade.quantity, trade.price_per_share, trade.direction)
        .map_err(|e: PnlError| anyhow::anyhow!("FIFO processing error: {e}"))?;

    let row = trade.to_db_values(&result)?;
    persist_metrics_row(pool, &row).await
}

async fn process_iteration(pool: &SqlitePool) -> anyhow::Result<usize> {
    let checkpoint = load_checkpoint(pool).await?;
    let all_trades = load_all_trades(pool).await?;
    let mut inventories = rebuild_fifo_state(&all_trades, checkpoint)?;

    let new_trades: Vec<_> = all_trades
        .into_iter()
        .filter(|t| t.timestamp > checkpoint)
        .collect();

    for trade in &new_trades {
        process_and_persist_trade(pool, &mut inventories, trade).await?;
    }

    Ok(new_trades.len())
}

pub async fn run(env: ReporterEnv) -> anyhow::Result<()> {
    use crate::env::HasSqlite;

    let pool = env.get_sqlite_pool().await?;
    let interval = env.processing_interval();

    info!("Starting P&L reporter");
    sqlx::migrate!().run(&pool).await?;

    info!(
        "Reporter initialized with processing interval: {}s",
        interval.as_secs()
    );

    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                match result {
                    Ok(()) => info!("Shutdown signal received"),
                    Err(e) => error!("Error receiving shutdown signal: {e}"),
                }
                break;
            }
            () = tokio::time::sleep(interval) => {
                match process_iteration(&pool).await {
                    Ok(count) => info!("Processed {count} new trades"),
                    Err(e) => error!("Processing error: {e}"),
                }
            }
        }
    }

    info!("Reporter shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    async fn create_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_load_checkpoint_empty_database() {
        let pool = create_test_pool().await;
        let checkpoint = load_checkpoint(&pool).await.unwrap();
        assert_eq!(checkpoint, DateTime::UNIX_EPOCH);
    }

    #[tokio::test]
    async fn test_rebuild_fifo_state_empty() {
        let trades: Vec<Trade> = vec![];
        let checkpoint = DateTime::UNIX_EPOCH;
        let inventories = rebuild_fifo_state(&trades, checkpoint).unwrap();
        assert!(inventories.is_empty());
    }

    #[tokio::test]
    async fn test_trade_from_onchain_row() {
        let naive_dt = DateTime::from_timestamp(0, 0).unwrap().naive_utc();

        let trade = Trade::from_onchain_row(
            1,
            "AAPL".to_string(),
            10.0,
            "BUY".to_string(),
            100.0,
            Some(naive_dt),
        )
        .unwrap();

        assert_eq!(trade.id, 1);
        assert_eq!(trade.symbol.as_str(), "AAPL");
        assert_eq!(trade.quantity, dec!(10.0));
        assert_eq!(trade.price_per_share, dec!(100.0));
        assert_eq!(trade.direction, Direction::Buy);
    }

    #[tokio::test]
    async fn test_trade_from_offchain_row() {
        let naive_dt = DateTime::from_timestamp(0, 0).unwrap().naive_utc();

        let trade = Trade::from_offchain_row(
            2,
            "AAPL".to_string(),
            5,
            "SELL".to_string(),
            Some(10500),
            Some(naive_dt),
        )
        .unwrap();

        assert_eq!(trade.id, 2);
        assert_eq!(trade.symbol.as_str(), "AAPL");
        assert_eq!(trade.quantity, dec!(5));
        assert_eq!(trade.price_per_share, dec!(105.00));
        assert_eq!(trade.direction, Direction::Sell);
    }

    #[tokio::test]
    async fn test_process_iteration_no_trades() {
        let pool = create_test_pool().await;
        let count = process_iteration(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
