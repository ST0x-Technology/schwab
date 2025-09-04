use chrono::{Duration, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::time::{Duration as TokioDuration, sleep};
use tracing::{debug, info, warn};

use crate::schwab::market_hours::MarketStatus;
use crate::schwab::market_hours_cache::MarketHoursCache;
use crate::schwab::{SchwabAuthEnv, SchwabError};

/// Buffer time in minutes to start bot before market opens.
const MARKET_OPEN_BUFFER_MINUTES: i64 = 5;

/// Buffer time in minutes to stop bot after market closes.
const MARKET_CLOSE_BUFFER_MINUTES: i64 = 5;

/// Market ID for equity markets.
const MARKET_ID: &str = "equity";

/// Trading Hours Controller manages when the arbitrage bot should run based on market hours.
///
/// This controller determines the appropriate times to start and stop the bot based on
/// Schwab market hours with configurable buffer times. It provides methods to check
/// current trading status and wait for market events.
#[derive(Debug)]
pub(crate) struct TradingHoursController {
    cache: Arc<MarketHoursCache>,
    env: SchwabAuthEnv,
    pool: Arc<SqlitePool>,
}

impl TradingHoursController {
    /// Create a new Trading Hours Controller.
    ///
    /// # Arguments
    /// * `cache` - Shared market hours cache for efficient API access
    /// * `env` - Schwab authentication environment configuration
    /// * `pool` - Database connection pool
    pub(crate) fn new(
        cache: Arc<MarketHoursCache>,
        env: SchwabAuthEnv,
        pool: Arc<SqlitePool>,
    ) -> Self {
        Self { cache, env, pool }
    }

    /// Determine if the bot should be running right now.
    ///
    /// Returns true if the current time falls within market hours plus buffer times.
    /// Buffer logic:
    /// - Bot starts 5 minutes before market opens
    /// - Bot stops 5 minutes after market closes
    ///
    /// # Errors
    /// Returns `SchwabError` if unable to fetch market hours from API.
    pub(crate) async fn should_bot_run(&self) -> Result<bool, SchwabError> {
        let status = self
            .cache
            .get_current_status(MARKET_ID, &self.env, &self.pool)
            .await?;

        match status {
            MarketStatus::Open => {
                debug!("Market is currently open, bot should run");
                Ok(true)
            }
            MarketStatus::Closed => {
                // Check if we're within buffer time before market opens
                if let Some(next_transition) = self
                    .cache
                    .get_next_transition(MARKET_ID, &self.env, &self.pool)
                    .await?
                {
                    let now = Utc::now();
                    let buffer_duration = Duration::minutes(MARKET_OPEN_BUFFER_MINUTES);
                    let start_time = next_transition - buffer_duration;

                    if now >= start_time {
                        debug!(
                            "Market is closed but within buffer time before open, bot should run"
                        );
                        Ok(true)
                    } else {
                        debug!("Market is closed and outside buffer time, bot should not run");
                        Ok(false)
                    }
                } else {
                    debug!("Market is closed with no upcoming transitions, bot should not run");
                    Ok(false)
                }
            }
        }
    }

    /// Wait until the market opens (including buffer time).
    ///
    /// This method will block until it's appropriate for the bot to start running.
    /// It accounts for the pre-market buffer time, so the bot will start running
    /// 5 minutes before the actual market open.
    ///
    /// # Errors
    /// Returns `SchwabError` if unable to fetch market hours or calculate wait times.
    pub(crate) async fn wait_until_market_open(&self) -> Result<(), SchwabError> {
        info!("Waiting for market to open...");

        loop {
            if self.should_bot_run().await? {
                info!("Market is now open (including buffer time), starting bot");
                return Ok(());
            }

            // Get next transition time and calculate sleep duration
            if let Some(next_transition) = self
                .cache
                .get_next_transition(MARKET_ID, &self.env, &self.pool)
                .await?
            {
                let now = Utc::now();
                let buffer_duration = Duration::minutes(MARKET_OPEN_BUFFER_MINUTES);
                let start_time = next_transition - buffer_duration;

                if start_time > now {
                    let sleep_duration = (start_time - now)
                        .to_std()
                        .unwrap_or(std::time::Duration::from_secs(60));

                    if sleep_duration > std::time::Duration::from_secs(300) {
                        // 5 minutes
                        info!(
                            "Market opens at {} (with buffer), sleeping for {} minutes",
                            start_time.format("%Y-%m-%d %H:%M:%S UTC"),
                            sleep_duration.as_secs() / 60
                        );
                    } else {
                        debug!(
                            "Market opens soon, sleeping for {} seconds",
                            sleep_duration.as_secs()
                        );
                    }

                    sleep(
                        TokioDuration::try_from(sleep_duration)
                            .unwrap_or(TokioDuration::from_secs(60)),
                    )
                    .await;
                    continue;
                }
            }

            // Fallback: sleep for 1 minute and check again
            warn!("Unable to determine next market open time, sleeping for 1 minute");
            sleep(TokioDuration::from_secs(60)).await;
        }
    }

    /// Get the duration until the market closes (including buffer time).
    ///
    /// Returns the time remaining until the bot should stop running. This includes
    /// the post-market buffer time, so the bot will continue running for 5 minutes
    /// after the actual market close.
    ///
    /// Returns `None` if the market is already closed or if unable to determine close time.
    ///
    /// # Errors  
    /// Returns `SchwabError` if unable to fetch market hours.
    pub(crate) async fn time_until_market_close(&self) -> Result<Option<Duration>, SchwabError> {
        let status = self
            .cache
            .get_current_status(MARKET_ID, &self.env, &self.pool)
            .await?;

        if status == MarketStatus::Closed {
            debug!("Market is already closed");
            return Ok(None);
        }

        if let Some(next_transition) = self
            .cache
            .get_next_transition(MARKET_ID, &self.env, &self.pool)
            .await?
        {
            let now = Utc::now();
            let buffer_duration = Duration::minutes(MARKET_CLOSE_BUFFER_MINUTES);
            let stop_time = next_transition + buffer_duration;

            if stop_time > now {
                let duration_until_close = stop_time - now;
                debug!(
                    "Market closes in {} minutes (including buffer)",
                    duration_until_close.num_minutes()
                );
                Ok(Some(duration_until_close))
            } else {
                debug!("Market close time with buffer has already passed");
                Ok(None)
            }
        } else {
            warn!("Unable to determine market close time");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schwab::tokens::SchwabTokens;
    use crate::test_utils::setup_test_db;
    use chrono::TimeZone;
    use chrono_tz::US::Eastern;
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
    async fn test_should_bot_run_market_open() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

        // Create market hours for today that would be currently open
        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let now = Utc::now().with_timezone(&Eastern);

        // Set market hours that span current time
        let start_time = now - Duration::hours(2);
        let end_time = now + Duration::hours(2);

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

        let result = controller.should_bot_run().await.unwrap();
        mock.assert();
        assert!(result);
    }

    #[tokio::test]
    async fn test_should_bot_run_market_closed_outside_buffer() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let tomorrow = today.succ_opt().unwrap();

        let today_mock_response = json!({
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

        let tomorrow_mock_response = json!({
            "equity": {
                "EQ": {
                    "date": tomorrow.format("%Y-%m-%d").to_string(),
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": false
                }
            }
        });

        // Mock today's market status
        let today_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", &today.format("%Y-%m-%d").to_string());
            then.status(200).json_body(today_mock_response);
        });

        // Mock tomorrow's market status for get_next_transition
        let tomorrow_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", &tomorrow.format("%Y-%m-%d").to_string());
            then.status(200).json_body(tomorrow_mock_response);
        });

        let result = controller.should_bot_run().await.unwrap();
        today_mock.assert();
        tomorrow_mock.assert();
        assert!(!result);
    }

    #[tokio::test]
    async fn test_should_bot_run_within_buffer_time() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let now = Utc::now().with_timezone(&Eastern);

        // Set market to open in 3 minutes (within 5 minute buffer)
        let market_open_time = now + Duration::minutes(3);
        let market_close_time = market_open_time + Duration::hours(6);

        // Mock today's market as closed
        let today_mock_response = json!({
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

        // Mock tomorrow's market hours
        let tomorrow = today.succ_opt().unwrap();
        let tomorrow_mock_response = json!({
            "equity": {
                "EQ": {
                    "date": tomorrow.format("%Y-%m-%d").to_string(),
                    "marketType": "EQUITY",
                    "exchange": "NYSE",
                    "category": "EQUITY",
                    "product": "EQ",
                    "productName": "Equity",
                    "isOpen": true,
                    "sessionHours": {
                        "regularMarket": [{
                            "start": market_open_time.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
                            "end": market_close_time.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
                        }]
                    }
                }
            }
        });

        let today_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", &today.format("%Y-%m-%d").to_string());
            then.status(200).json_body(today_mock_response);
        });

        let tomorrow_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/marketdata/v1/markets/equity")
                .query_param("date", &tomorrow.format("%Y-%m-%d").to_string());
            then.status(200).json_body(tomorrow_mock_response);
        });

        // First call should check today's status
        let result = controller.should_bot_run().await.unwrap();

        today_mock.assert();
        tomorrow_mock.assert();

        // Since we're within 5 minutes of market open, bot should run
        assert!(result);
    }

    #[tokio::test]
    async fn test_time_until_market_close_open_market() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

        let today = Eastern
            .from_utc_datetime(&Utc::now().naive_utc())
            .date_naive();
        let now = Utc::now().with_timezone(&Eastern);

        // Market is open and closes in 2 hours
        let start_time = now - Duration::hours(2);
        let end_time = now + Duration::hours(2);

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

        let result = controller.time_until_market_close().await.unwrap();
        mock.assert();

        assert!(result.is_some());
        let duration = result.unwrap();

        // Should be approximately 2 hours + 5 minute buffer = 125 minutes
        assert!(duration.num_minutes() >= 120); // At least 2 hours
        assert!(duration.num_minutes() <= 130); // At most 2 hours 10 minutes (allowing for test timing)
    }

    #[tokio::test]
    async fn test_time_until_market_close_closed_market() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

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

        let result = controller.time_until_market_close().await.unwrap();
        mock.assert();

        // Market is closed, should return None
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_buffer_time_constants() {
        // Verify buffer times are set as required
        assert_eq!(MARKET_OPEN_BUFFER_MINUTES, 5);
        assert_eq!(MARKET_CLOSE_BUFFER_MINUTES, 5);
        assert_eq!(MARKET_ID, "equity");
    }

    #[tokio::test]
    async fn test_api_error_propagation() {
        let server = MockServer::start();
        let env = create_test_env_with_mock_server(&server);
        let pool = Arc::new(setup_test_db().await);
        setup_test_tokens(&pool).await;

        let cache = Arc::new(MarketHoursCache::new());
        let controller = TradingHoursController::new(cache, env, pool);

        let mock = server.mock(|when, then| {
            when.method(GET).path("/marketdata/v1/markets/equity");
            then.status(500).body("Internal server error");
        });

        let result = controller.should_bot_run().await;
        mock.assert();

        assert!(matches!(
            result.unwrap_err(),
            SchwabError::RequestFailed { action, status, .. }
            if action == "fetch market hours" && status.as_u16() == 500
        ));
    }
}
