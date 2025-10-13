use alloy::primitives::FixedBytes;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::schwab::auth::SchwabAuthEnv;
use crate::schwab::tokens::SchwabTokens;

pub const TEST_ENCRYPTION_KEY: FixedBytes<32> = FixedBytes::ZERO;

/// Centralized test database setup for broker crate tests.
/// Creates an in-memory SQLite database with all migrations applied.
pub async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    pool
}

/// Set up valid test tokens in the database for testing.
pub async fn setup_test_tokens(pool: &SqlitePool, env: &SchwabAuthEnv) {
    let tokens = SchwabTokens {
        access_token: "test_access_token".to_string(),
        access_token_fetched_at: Utc::now(),
        refresh_token: "test_refresh_token".to_string(),
        refresh_token_fetched_at: Utc::now(),
    };
    tokens.store(pool, &env.encryption_key).await.unwrap();
}
