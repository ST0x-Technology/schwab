use sqlx::SqlitePool;

/// Centralized test database setup for broker crate tests.
/// Creates an in-memory SQLite database with all migrations applied.
pub async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    pool
}
