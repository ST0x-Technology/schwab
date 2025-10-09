use chrono::{DateTime, Utc};
use rain_schwab::reporter::process_iteration;
use sqlx::SqlitePool;

fn assert_f64_eq(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < f64::EPSILON,
        "assertion failed: `(left == right)` (left: `{actual}`, right: `{expected}`)"
    );
}

fn assert_option_f64_eq(actual: Option<f64>, expected: Option<f64>) {
    match (actual, expected) {
        (Some(a), Some(e)) => assert_f64_eq(a, e),
        (None, None) => (),
        _ => panic!(
            "assertion failed: `(left == right)` (left: `{actual:?}`, right: `{expected:?}`)"
        ),
    }
}

async fn create_test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:")
        .await
        .expect("Failed to connect to in-memory database");

    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(&pool)
        .await
        .expect("Failed to set WAL mode");

    sqlx::query("PRAGMA busy_timeout = 10000")
        .execute(&pool)
        .await
        .expect("Failed to set busy timeout");

    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");
    pool
}

async fn insert_onchain_trade(
    pool: &SqlitePool,
    symbol: &str,
    amount: f64,
    price_usdc: f64,
    direction: &str,
    timestamp: DateTime<Utc>,
) {
    let tx_hash = format!("0x{:064x}", rand::random::<u64>());
    let log_index = i64::try_from(rand::random::<u64>() % 1000).expect("log_index overflow");
    let naive_timestamp = timestamp.naive_utc();

    sqlx::query!(
        "INSERT INTO onchain_trades (
            tx_hash,
            log_index,
            symbol,
            amount,
            direction,
            price_usdc,
            created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        tx_hash,
        log_index,
        symbol,
        amount,
        direction,
        price_usdc,
        naive_timestamp,
    )
    .execute(pool)
    .await
    .expect("Failed to insert onchain trade");
}

async fn insert_offchain_trade(
    pool: &SqlitePool,
    symbol: &str,
    shares: i64,
    direction: &str,
    price_cents: i64,
    timestamp: DateTime<Utc>,
) {
    let order_id = format!("ORDER{}", rand::random::<u32>());
    let status = "FILLED";
    let naive_timestamp = timestamp.naive_utc();

    sqlx::query!(
        "INSERT INTO schwab_executions (
            order_id,
            symbol,
            shares,
            direction,
            price_cents,
            status,
            executed_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        order_id,
        symbol,
        shares,
        direction,
        price_cents,
        status,
        naive_timestamp,
    )
    .execute(pool)
    .await
    .expect("Failed to insert offchain trade");
}

struct PnlMetric {
    realized_pnl: Option<f64>,
    cumulative_pnl: f64,
    net_position_after: f64,
    trade_type: String,
}

async fn query_all_pnl_metrics(pool: &SqlitePool, symbol: &str) -> Vec<PnlMetric> {
    let rows = sqlx::query!(
        "SELECT
            trade_type,
            realized_pnl,
            cumulative_pnl,
            net_position_after
        FROM metrics_pnl
        WHERE symbol = ?
        ORDER BY timestamp ASC",
        symbol
    )
    .fetch_all(pool)
    .await
    .expect("Failed to query pnl metrics");

    rows.into_iter()
        .map(|r| PnlMetric {
            trade_type: r.trade_type,
            realized_pnl: r.realized_pnl,
            cumulative_pnl: r.cumulative_pnl,
            net_position_after: r.net_position_after,
        })
        .collect()
}

#[tokio::test]
async fn test_simple_buy_sell_end_to_end() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_onchain_trade(&pool, "AAPL", 100.0, 11.0, "SELL", t2).await;

    let count = process_iteration(&pool)
        .await
        .expect("Failed to process iteration");
    assert_eq!(count, 2);

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 2);

    assert_option_f64_eq(metrics[0].realized_pnl, None);
    assert_f64_eq(metrics[0].net_position_after, 100.0);

    assert_option_f64_eq(metrics[1].realized_pnl, Some(100.0));
    assert_f64_eq(metrics[1].cumulative_pnl, 100.0);
    assert_f64_eq(metrics[1].net_position_after, 0.0);
}

#[tokio::test]
async fn test_multiple_trades_fifo_ordering() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");
    let t3 = DateTime::from_timestamp(3000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_onchain_trade(&pool, "AAPL", 50.0, 12.0, "BUY", t2).await;
    insert_onchain_trade(&pool, "AAPL", 80.0, 11.0, "SELL", t3).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 3);

    assert_option_f64_eq(metrics[2].realized_pnl, Some(80.0));
    assert_f64_eq(metrics[2].cumulative_pnl, 80.0);
    assert_f64_eq(metrics[2].net_position_after, 70.0);
}

#[tokio::test]
async fn test_position_reversal() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_onchain_trade(&pool, "AAPL", 150.0, 11.0, "SELL", t2).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 2);

    assert_option_f64_eq(metrics[1].realized_pnl, Some(100.0));
    assert_f64_eq(metrics[1].net_position_after, -50.0);
}

#[tokio::test]
async fn test_checkpoint_resume() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");
    let t3 = DateTime::from_timestamp(3000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_onchain_trade(&pool, "AAPL", 100.0, 11.0, "SELL", t2).await;

    let count = process_iteration(&pool)
        .await
        .expect("Failed to process iteration");
    assert_eq!(count, 2);

    insert_onchain_trade(&pool, "AAPL", 50.0, 12.0, "BUY", t3).await;

    let count = process_iteration(&pool)
        .await
        .expect("Failed to process iteration");
    assert_eq!(count, 1);

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 3);

    assert_f64_eq(metrics[2].net_position_after, 50.0);
}

#[tokio::test]
async fn test_mixed_onchain_offchain_trades() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");
    let t3 = DateTime::from_timestamp(3000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_offchain_trade(&pool, "AAPL", 50, "SELL", 1100, t2).await;
    insert_onchain_trade(&pool, "AAPL", 30.0, 12.0, "SELL", t3).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 3);

    assert_eq!(metrics[1].trade_type, "OFFCHAIN");
    assert_option_f64_eq(metrics[1].realized_pnl, Some(50.0));

    assert_option_f64_eq(metrics[2].realized_pnl, Some(60.0));
    assert_f64_eq(metrics[2].net_position_after, 20.0);
}

#[tokio::test]
async fn test_multiple_symbols_independent() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");
    let t2 = DateTime::from_timestamp(2000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;
    insert_onchain_trade(&pool, "MSFT", 50.0, 200.0, "BUY", t1).await;

    insert_onchain_trade(&pool, "AAPL", 100.0, 12.0, "SELL", t2).await;
    insert_onchain_trade(&pool, "MSFT", 50.0, 210.0, "SELL", t2).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    let aapl_metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(aapl_metrics.len(), 2);
    assert_f64_eq(aapl_metrics[1].cumulative_pnl, 200.0);

    let msft_metrics = query_all_pnl_metrics(&pool, "MSFT").await;
    assert_eq!(msft_metrics.len(), 2);
    assert_f64_eq(msft_metrics[1].cumulative_pnl, 500.0);
}

#[tokio::test]
async fn test_duplicate_prevention() {
    let pool = create_test_pool().await;

    let t1 = DateTime::from_timestamp(1000, 0).expect("Invalid timestamp");

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", t1).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");
    let count = process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    assert_eq!(count, 0);

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 1);
}

#[tokio::test]
async fn test_requirements_doc_seven_step_example() {
    let pool = create_test_pool().await;

    let mut time = 1000;
    let mut next_time = || {
        time += 1000;
        DateTime::from_timestamp(time, 0).expect("Invalid timestamp")
    };

    insert_onchain_trade(&pool, "AAPL", 100.0, 10.0, "BUY", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 50.0, 12.0, "BUY", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 80.0, 11.0, "SELL", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 60.0, 9.5, "SELL", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 30.0, 12.2, "BUY", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 70.0, 12.0, "SELL", next_time()).await;
    insert_onchain_trade(&pool, "AAPL", 20.0, 11.5, "BUY", next_time()).await;

    process_iteration(&pool)
        .await
        .expect("Failed to process iteration");

    let metrics = query_all_pnl_metrics(&pool, "AAPL").await;
    assert_eq!(metrics.len(), 7);

    assert_option_f64_eq(metrics[0].realized_pnl, None);
    assert_f64_eq(metrics[0].cumulative_pnl, 0.0);
    assert_f64_eq(metrics[0].net_position_after, 100.0);

    assert_option_f64_eq(metrics[1].realized_pnl, None);
    assert_f64_eq(metrics[1].cumulative_pnl, 0.0);
    assert_f64_eq(metrics[1].net_position_after, 150.0);

    assert_option_f64_eq(metrics[2].realized_pnl, Some(80.0));
    assert_f64_eq(metrics[2].cumulative_pnl, 80.0);
    assert_f64_eq(metrics[2].net_position_after, 70.0);

    assert_option_f64_eq(metrics[3].realized_pnl, Some(-110.0));
    assert_f64_eq(metrics[3].cumulative_pnl, -30.0);
    assert_f64_eq(metrics[3].net_position_after, 10.0);

    assert_option_f64_eq(metrics[4].realized_pnl, None);
    assert_f64_eq(metrics[4].cumulative_pnl, -30.0);
    assert_f64_eq(metrics[4].net_position_after, 40.0);

    assert_option_f64_eq(metrics[5].realized_pnl, Some(-6.0));
    assert_f64_eq(metrics[5].cumulative_pnl, -36.0);
    assert_f64_eq(metrics[5].net_position_after, -30.0);

    assert_option_f64_eq(metrics[6].realized_pnl, Some(10.0));
    assert_f64_eq(metrics[6].cumulative_pnl, -26.0);
    assert_f64_eq(metrics[6].net_position_after, -10.0);
}
