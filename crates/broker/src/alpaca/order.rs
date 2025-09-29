use apca::api::v2::order;
use apca::{Client, RequestError};
use tracing::debug;

use crate::{BrokerError, Direction, MarketOrder, OrderPlacement, Shares, Symbol};

pub(super) async fn place_market_order(
    client: &Client,
    market_order: MarketOrder,
) -> Result<OrderPlacement<String>, BrokerError> {
    debug!(
        "Placing Alpaca market order: {} {} shares of {}",
        market_order.direction, market_order.shares, market_order.symbol
    );

    let alpaca_side = match market_order.direction {
        Direction::Buy => order::Side::Buy,
        Direction::Sell => order::Side::Sell,
    };

    let order_init = order::CreateReqInit {
        class: order::Class::Simple,
        type_: order::Type::Market,
        time_in_force: order::TimeInForce::Day,
        extended_hours: false,
        ..Default::default()
    };

    let order_request = order_init.init(
        market_order.symbol.to_string(),
        alpaca_side,
        order::Amount::quantity(market_order.shares.value()),
    );

    let order_response = client
        .issue::<order::Create>(&order_request)
        .await
        .map_err(|e| match e {
            RequestError::Endpoint(endpoint_error) => {
                BrokerError::AlpacaRequest(format!("Order placement failed: {}", endpoint_error))
            }
            RequestError::Hyper(hyper_error) => {
                BrokerError::AlpacaRequest(format!("HTTP error: {}", hyper_error))
            }
            RequestError::HyperUtil(hyper_util_error) => {
                BrokerError::AlpacaRequest(format!("HTTP util error: {}", hyper_util_error))
            }
            RequestError::Io(io_error) => {
                BrokerError::AlpacaRequest(format!("IO error: {}", io_error))
            }
        })?;

    let order_id = order_response.id.to_string();

    Ok(OrderPlacement {
        order_id,
        symbol: market_order.symbol,
        shares: market_order.shares,
        direction: market_order.direction,
        placed_at: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn create_test_client(mock_server: &MockServer) -> Client {
        let api_info =
            apca::ApiInfo::from_parts(&mock_server.base_url(), "test_key", "test_secret").unwrap();
        Client::new(api_info)
    }

    #[tokio::test]
    async fn test_place_market_order_buy_success() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders").json_body(json!({
                "symbol": "AAPL",
                "qty": "100",
                "side": "buy",
                "type": "market",
                "time_in_force": "day",
                "order_class": "simple",
                "extended_hours": false,
                "client_order_id": null,
                "limit_price": null,
                "stop_price": null,
                "trail_price": null,
                "trail_percent": null,
                "take_profit": null,
                "stop_loss": null
            }));
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "id": "904837e3-3b76-47ec-b432-046db621571b",
                    "client_order_id": "",
                    "symbol": "AAPL",
                    "asset_id": "904837e3-3b76-47ec-b432-046db621571b",
                    "asset_class": "us_equity",
                    "qty": "100",
                    "filled_qty": "0",
                    "side": "buy",
                    "order_class": "simple",
                    "type": "market",
                    "time_in_force": "day",
                    "limit_price": null,
                    "stop_price": null,
                    "trail_price": null,
                    "trail_percent": null,
                    "status": "new",
                    "extended_hours": false,
                    "legs": [],
                    "created_at": "2030-01-15T09:30:00.000Z",
                    "updated_at": null,
                    "submitted_at": null,
                    "filled_at": null,
                    "expired_at": null,
                    "canceled_at": null,
                    "average_fill_price": null
                }));
        });

        let client = create_test_client(&server);
        let market_order = MarketOrder {
            symbol: Symbol::new("AAPL".to_string()).unwrap(),
            shares: Shares::new(100).unwrap(),
            direction: Direction::Buy,
        };

        let result = place_market_order(&client, market_order).await;

        mock.assert();
        let placement = result.unwrap();
        assert_eq!(placement.order_id, "904837e3-3b76-47ec-b432-046db621571b");
        assert_eq!(placement.symbol.to_string(), "AAPL");
        assert_eq!(placement.shares.value(), 100);
        assert_eq!(placement.direction, Direction::Buy);
    }

    #[tokio::test]
    async fn test_place_market_order_sell_success() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders").json_body(json!({
                "symbol": "TSLA",
                "qty": "50",
                "side": "sell",
                "type": "market",
                "time_in_force": "day",
                "order_class": "simple",
                "extended_hours": false,
                "client_order_id": null,
                "limit_price": null,
                "stop_price": null,
                "trail_price": null,
                "trail_percent": null,
                "take_profit": null,
                "stop_loss": null
            }));
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "id": "61e7b016-9c91-4a97-b912-615c9d365c9d",
                    "client_order_id": "",
                    "symbol": "TSLA",
                    "asset_id": "61e7b016-9c91-4a97-b912-615c9d365c9d",
                    "asset_class": "us_equity",
                    "qty": "50",
                    "filled_qty": "0",
                    "side": "sell",
                    "order_class": "simple",
                    "type": "market",
                    "time_in_force": "day",
                    "limit_price": null,
                    "stop_price": null,
                    "trail_price": null,
                    "trail_percent": null,
                    "status": "new",
                    "extended_hours": false,
                    "legs": [],
                    "created_at": "2030-01-15T09:30:00.000Z",
                    "updated_at": null,
                    "submitted_at": null,
                    "filled_at": null,
                    "expired_at": null,
                    "canceled_at": null,
                    "average_fill_price": null
                }));
        });

        let client = create_test_client(&server);
        let market_order = MarketOrder {
            symbol: Symbol::new("TSLA".to_string()).unwrap(),
            shares: Shares::new(50).unwrap(),
            direction: Direction::Sell,
        };

        let result = place_market_order(&client, market_order).await;

        mock.assert();
        let placement = result.unwrap();
        assert_eq!(placement.order_id, "61e7b016-9c91-4a97-b912-615c9d365c9d");
        assert_eq!(placement.symbol.to_string(), "TSLA");
        assert_eq!(placement.shares.value(), 50);
        assert_eq!(placement.direction, Direction::Sell);
    }

    #[tokio::test]
    async fn test_place_market_order_invalid_symbol() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(422)
                .header("content-type", "application/json")
                .json_body(json!({
                    "code": 40010001,
                    "message": "symbol INVALID is not supported"
                }));
        });

        let client = create_test_client(&server);
        let market_order = MarketOrder {
            symbol: Symbol::new("INVALID".to_string()).unwrap(),
            shares: Shares::new(10).unwrap(),
            direction: Direction::Buy,
        };

        let result = place_market_order(&client, market_order).await;

        mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(error, BrokerError::AlpacaRequest(_)));
    }

    #[tokio::test]
    async fn test_place_market_order_authentication_failure() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(401)
                .header("content-type", "application/json")
                .json_body(json!({
                    "code": 40110000,
                    "message": "Invalid credentials"
                }));
        });

        let client = create_test_client(&server);
        let market_order = MarketOrder {
            symbol: Symbol::new("AAPL".to_string()).unwrap(),
            shares: Shares::new(100).unwrap(),
            direction: Direction::Buy,
        };

        let result = place_market_order(&client, market_order).await;

        mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(error, BrokerError::AlpacaRequest(_)));
    }

    #[tokio::test]
    async fn test_place_market_order_server_error() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(POST).path("/v2/orders");
            then.status(500)
                .header("content-type", "application/json")
                .json_body(json!({
                    "message": "Internal server error"
                }));
        });

        let client = create_test_client(&server);
        let market_order = MarketOrder {
            symbol: Symbol::new("SPY".to_string()).unwrap(),
            shares: Shares::new(25).unwrap(),
            direction: Direction::Buy,
        };

        let result = place_market_order(&client, market_order).await;

        mock.assert();
        let error = result.unwrap_err();
        assert!(matches!(error, BrokerError::AlpacaRequest(_)));
    }
}
