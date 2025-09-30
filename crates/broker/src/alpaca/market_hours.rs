use apca::api::v2::clock;
use apca::{Client, RequestError};
use chrono::Utc;
use tracing::debug;

pub(super) async fn wait_until_market_open(
    client: &Client,
) -> Result<Option<std::time::Duration>, RequestError<clock::GetError>> {
    debug!("Checking market status via Alpaca Clock API");

    let clock_data = client.issue::<clock::Get>(&()).await?;

    if clock_data.open {
        Ok(None)
    } else {
        let now = Utc::now();
        let next_open_utc = clock_data.next_open;

        if next_open_utc > now {
            let chrono_duration = next_open_utc - now;
            match chrono_duration.to_std() {
                Ok(duration) => Ok(Some(duration)),
                Err(_) => {
                    // Duration is negative or out of range, market should be open
                    debug!("Duration conversion failed, treating as market open");
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn create_test_client(mock_server: &MockServer) -> Client {
        let api_info =
            apca::ApiInfo::from_parts(mock_server.base_url(), "test_key", "test_secret").unwrap();
        Client::new(api_info)
    }

    #[tokio::test]
    async fn test_wait_until_market_open_when_open() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/clock");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "timestamp": "2025-01-03T14:30:00-05:00",
                    "is_open": true,
                    "next_open": "2030-01-06T14:30:00+00:00",
                    "next_close": "2030-01-06T21:00:00+00:00"
                }));
        });

        let client = create_test_client(&server);
        let result = wait_until_market_open(&client).await;

        mock.assert();
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_wait_until_market_open_when_closed() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/clock");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "timestamp": "2025-01-03T20:00:00-05:00",
                    "is_open": false,
                    "next_open": "2030-01-06T14:30:00+00:00",
                    "next_close": "2030-01-06T21:00:00+00:00"
                }));
        });

        let client = create_test_client(&server);
        let result = wait_until_market_open(&client).await;

        mock.assert();
        let duration = result.unwrap().unwrap();
        assert!(duration.as_secs() > 0);
    }

    #[tokio::test]
    async fn test_wait_until_market_open_future_weekend() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/clock");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "timestamp": "2025-01-04T20:00:00-05:00",
                    "is_open": false,
                    "next_open": "2030-01-06T14:30:00+00:00",
                    "next_close": "2030-01-06T21:00:00+00:00"
                }));
        });

        let client = create_test_client(&server);
        let result = wait_until_market_open(&client).await;

        mock.assert();
        let duration = result.unwrap().unwrap();
        assert!(duration.as_secs() > 0);
    }
}
