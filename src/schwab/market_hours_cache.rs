use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::US::Eastern;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::trace;

use super::market_hours::{MarketHours, MarketStatus, fetch_market_hours};
use super::{SchwabAuthEnv, SchwabError};

/// Thread-safe cache for market hours data to reduce API calls.
///
/// Caches only today and tomorrow's market hours to minimize memory usage.
/// Uses async RwLock for thread-safe concurrent access.
#[derive(Debug)]
pub(crate) struct MarketHoursCache {
    cache: RwLock<HashMap<(String, NaiveDate), MarketHours>>,
}

impl Default for MarketHoursCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketHoursCache {
    /// Create a new empty market hours cache.
    pub(crate) fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Get market hours for the specified market and date, fetching from API if not cached.
    ///
    /// This method checks the cache first and only makes an API call if the data is missing
    /// or stale. All API failures are propagated without fallback values.
    pub(crate) async fn get_or_fetch(
        &self,
        market_id: &str,
        date: NaiveDate,
        env: &SchwabAuthEnv,
        pool: &SqlitePool,
    ) -> Result<MarketHours, SchwabError> {
        let cache_key = (market_id.to_string(), date);

        // Check cache first (read lock)
        {
            let cache_guard = self.cache.read().await;
            if let Some(market_hours) = cache_guard.get(&cache_key) {
                trace!("Cache hit for market {} on {}", market_id, date);
                return Ok(market_hours.clone());
            }
        }

        // Cache miss - fetch from API
        trace!(
            "Cache miss for market {} on {}, fetching from API",
            market_id, date
        );
        let date_str = date.format("%Y-%m-%d").to_string();
        let market_hours = fetch_market_hours(env, pool, Some(&date_str)).await?;

        // Store in cache (write lock)
        {
            let mut cache_guard = self.cache.write().await;
            cache_guard.insert(cache_key, market_hours.clone());
        }

        Ok(market_hours)
    }

    /// Get current market status for the specified market.
    ///
    /// Returns the current status (Open/Closed) based on today's market hours
    /// and the current time in Eastern timezone.
    pub(crate) async fn get_current_status(
        &self,
        market_id: &str,
        env: &SchwabAuthEnv,
        pool: &SqlitePool,
    ) -> Result<MarketStatus, SchwabError> {
        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let market_hours = self.get_or_fetch(market_id, today, env, pool).await?;
        Ok(market_hours.current_status())
    }

    /// Get the next market transition time (open or close).
    ///
    /// Returns the next time the market will change state (open to closed or closed to open).
    /// If the market is currently open, returns the close time. If closed, returns the next open time.
    pub(crate) async fn get_next_transition(
        &self,
        market_id: &str,
        env: &SchwabAuthEnv,
        pool: &SqlitePool,
    ) -> Result<Option<DateTime<Utc>>, SchwabError> {
        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let today_hours = self.get_or_fetch(market_id, today, env, pool).await?;
        let now = Utc::now().with_timezone(&Eastern);

        // If market is open today, return today's close time
        if today_hours.current_status() == MarketStatus::Open {
            if let Some(end_time) = today_hours.end {
                return Ok(Some(end_time.with_timezone(&Utc)));
            }
        }

        // If market is closed, find next open time
        // Check if today's market hasn't opened yet
        if let Some(start_time) = today_hours.start {
            if now < start_time && today_hours.is_open {
                return Ok(Some(start_time.with_timezone(&Utc)));
            }
        }

        // Check tomorrow's market hours
        let tomorrow = today.succ_opt().ok_or_else(|| SchwabError::RequestFailed {
            action: "calculate tomorrow's date".to_string(),
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "Date overflow calculating tomorrow".to_string(),
        })?;

        let tomorrow_hours = self.get_or_fetch(market_id, tomorrow, env, pool).await?;
        if let Some(start_time) = tomorrow_hours.start {
            if tomorrow_hours.is_open {
                return Ok(Some(start_time.with_timezone(&Utc)));
            }
        }

        // No upcoming transition found in the next day
        Ok(None)
    }

    /// Get the number of entries currently in the cache.
    ///
    /// This method is primarily for testing and monitoring purposes.
    #[cfg(test)]
    pub(crate) async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::super::market_hours::MarketSession;
    use super::*;
    use crate::schwab::tokens::SchwabTokens;
    use crate::test_utils::setup_test_db;
    use chrono::TimeZone;
    use httpmock::prelude::*;
    use serde_json::json;

    fn create_test_env_with_mock_server(mock_server: &MockServer) -> SchwabAuthEnv {
        SchwabAuthEnv {
            app_key: "test_app_key".to_string(),
            app_secret: "test_app_secret".to_string(),
            redirect_uri: "https://127.0.0.1".to_string(),
            base_url: mock_server.base_url(),
            account_index: 0,
        }
    }

    async fn setup_test_tokens(pool: &SqlitePool) {
        let tokens = SchwabTokens {
            access_token: "test_access_token".to_string(),
            access_token_fetched_at: Utc::now(),
            refresh_token: "test_refresh_token".to_string(),
            refresh_token_fetched_at: Utc::now(),
        };
        tokens.store(pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let cache = MarketHoursCache::new();
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let test_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();
        let market_hours = MarketHours {
            date: test_date,
            session_type: MarketSession::Regular,
            start: Some(Eastern.with_ymd_and_hms(2025, 1, 3, 9, 30, 0).unwrap()),
            end: Some(Eastern.with_ymd_and_hms(2025, 1, 3, 16, 0, 0).unwrap()),
            is_open: true,
        };

        // Manually insert into cache
        {
            let mut cache_guard = cache.cache.write().await;
            cache_guard.insert(("equity".to_string(), test_date), market_hours.clone());
        }

        // Should return cached value without making API call
        let result = cache
            .get_or_fetch("equity", test_date, &env, &pool)
            .await
            .unwrap();
        assert_eq!(result.date, test_date);
        assert!(result.is_open);
        assert_eq!(result.session_type, MarketSession::Regular);
    }

    #[tokio::test]
    async fn test_cache_miss_with_api_fetch() {
        let cache = MarketHoursCache::new();
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock_response = json!({
            "equity": {
                "EQ": {
                    "date": "2025-01-03",
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": true,
                    "sessionHours": {
                        "regularMarket": [{
                            "start": "2025-01-03T09:30:00-05:00",
                            "end": "2025-01-03T16:00:00-05:00"
                        }]
                    }
                }
            }
        });

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", "2025-01-03")
                .header("authorization", "Bearer test_access_token");
            then.status(200).json_body(mock_response);
        });

        let test_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();
        let result = cache
            .get_or_fetch("equity", test_date, &env, &pool)
            .await
            .unwrap();

        mock.assert();
        assert_eq!(result.date, test_date);
        assert!(result.is_open);

        // Verify it's now cached
        assert_eq!(cache.cache_size().await, 1);
    }

    #[tokio::test]
    async fn test_get_current_status_open() {
        let cache = MarketHoursCache::new();
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        // Create market hours for today that would be currently open
        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let now = Utc::now().with_timezone(&Eastern);

        // Set market hours that span current time
        let start_time = now - chrono::Duration::hours(2);
        let end_time = now + chrono::Duration::hours(2);

        let mock_response = json!({
            "equity": {
                "EQ": {
                    "date": today.format("%Y-%m-%d").to_string(),
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": true,
                    "sessionHours": {
                        "regularMarket": [{
                            "start": start_time.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
                            "end": end_time.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
                        }]
                    }
                }
            }
        });

        let mock = server.mock(|when, then| {
            when.method(GET).path("/marketdata/v1/markets/equity");
            then.status(200).json_body(mock_response);
        });

        let status = cache
            .get_current_status("equity", &env, &pool)
            .await
            .unwrap();

        mock.assert();
        assert_eq!(status, MarketStatus::Open);
    }

    #[tokio::test]
    async fn test_get_current_status_closed() {
        let cache = MarketHoursCache::new();
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();

        let mock_response = json!({
            "equity": {
                "EQ": {
                    "date": today.format("%Y-%m-%d").to_string(),
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": false
                }
            }
        });

        let mock = server.mock(|when, then| {
            when.method(GET).path("/marketdata/v1/markets/equity");
            then.status(200).json_body(mock_response);
        });

        let status = cache
            .get_current_status("equity", &env, &pool)
            .await
            .unwrap();

        mock.assert();
        assert_eq!(status, MarketStatus::Closed);
    }

    #[tokio::test]
    async fn test_api_error_propagation() {
        let cache = MarketHoursCache::new();
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = setup_test_db().await;
        setup_test_tokens(&pool).await;

        let mock = server.mock(|when, then| {
            when.method(GET).path("/marketdata/v1/markets/equity");
            then.status(500).body("Internal server error");
        });

        let test_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();
        let result = cache.get_or_fetch("equity", test_date, &env, &pool).await;

        mock.assert();
        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "fetch market hours" && status.as_u16() == 500
        ));
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let cache = std::sync::Arc::new(MarketHoursCache::new());
        let server = MockServer::start();
        let env = std::sync::Arc::new(create_test_env_with_mock_server(&server));
        let pool = std::sync::Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let mock_response = json!({
            "equity": {
                "EQ": {
                    "date": "2025-01-03",
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": true,
                    "sessionHours": {
                        "regularMarket": [{
                            "start": "2025-01-03T09:30:00-05:00",
                            "end": "2025-01-03T16:00:00-05:00"
                        }]
                    }
                }
            }
        });

        // Only one API call should be made despite concurrent requests
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", "2025-01-03")
                .header("authorization", "Bearer test_access_token");
            then.status(200).json_body(mock_response);
        });

        let test_date = NaiveDate::from_ymd_opt(2025, 1, 3).unwrap();

        // First request should populate the cache
        let first_result = cache
            .get_or_fetch("equity", test_date, &env, &pool)
            .await
            .unwrap();
        assert_eq!(first_result.date, test_date);
        assert!(first_result.is_open);

        // Now launch multiple concurrent requests - these should all hit the cache
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let cache_clone = cache.clone();
                let env_clone = env.clone();
                let pool_clone = pool.clone();
                tokio::spawn(async move {
                    cache_clone
                        .get_or_fetch("equity", test_date, &env_clone, &pool_clone)
                        .await
                })
            })
            .collect();

        // Wait for all to complete and collect results
        for handle in handles {
            let result = handle.await.unwrap().unwrap();
            assert_eq!(result.date, test_date);
            assert!(result.is_open);
        }

        // Mock should be called exactly once (only the first request)
        mock.assert_hits(1);
    }
}
