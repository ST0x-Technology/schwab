use apca::api::v2::account::{self, GetError};
use apca::{Client, RequestError};
use clap::Parser;

/// Alpaca API authentication environment configuration
#[derive(Parser, Debug, Clone)]
pub struct AlpacaAuthEnv {
    /// Alpaca API key ID
    #[clap(long, env = "APCA_API_KEY_ID")]
    pub api_key_id: String,

    /// Alpaca API secret key
    #[clap(long, env = "APCA_API_SECRET_KEY")]
    pub api_secret_key: String,

    /// Alpaca API base URL (paper trading vs live)
    /// Paper: https://paper-api.alpaca.markets
    /// Live: https://api.alpaca.markets
    #[clap(
        long,
        env = "APCA_BASE_URL",
        default_value = "https://paper-api.alpaca.markets"
    )]
    pub base_url: String,
}

impl AlpacaAuthEnv {
    /// Returns true if this configuration is for paper trading
    pub fn is_paper_trading(&self) -> bool {
        self.base_url
            .starts_with("https://paper-api.alpaca.markets")
    }

    /// Returns true if this configuration is for live trading
    pub fn is_live_trading(&self) -> bool {
        self.base_url.starts_with("https://api.alpaca.markets")
    }
}

/// Alpaca API client wrapper with authentication and configuration
pub struct AlpacaClient {
    client: Client,
    is_paper_trading: bool,
}

impl AlpacaClient {
    /// Create a new AlpacaClient from configuration
    ///
    /// # Errors
    /// Returns `apca::Error` if API configuration is invalid
    pub fn new(env: &AlpacaAuthEnv) -> Result<Self, crate::BrokerError> {
        let api_info =
            apca::ApiInfo::from_parts(&env.base_url, &env.api_key_id, &env.api_secret_key)?;

        let client = Client::new(api_info);
        let is_paper_trading = env.is_paper_trading();

        Ok(Self {
            client,
            is_paper_trading,
        })
    }

    /// Verify account credentials by calling the account endpoint
    ///
    /// # Errors
    /// Returns `RequestError` if authentication fails or account cannot be retrieved
    pub async fn verify_account(&self) -> Result<(), RequestError<GetError>> {
        let _account = self.client.issue::<account::Get>(&()).await?;
        Ok(())
    }

    /// Returns true if this client is configured for paper trading
    pub fn is_paper_trading(&self) -> bool {
        self.is_paper_trading
    }

    /// Returns true if this client is configured for live trading
    pub fn is_live_trading(&self) -> bool {
        !self.is_paper_trading
    }

    /// Access the underlying apca Client
    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_paper_config() -> AlpacaAuthEnv {
        AlpacaAuthEnv {
            api_key_id: "test_key_id".to_string(),
            api_secret_key: "test_secret_key".to_string(),
            base_url: "https://paper-api.alpaca.markets".to_string(),
        }
    }

    fn create_test_live_config() -> AlpacaAuthEnv {
        AlpacaAuthEnv {
            api_key_id: "test_key_id".to_string(),
            api_secret_key: "test_secret_key".to_string(),
            base_url: "https://api.alpaca.markets".to_string(),
        }
    }

    #[test]
    fn test_alpaca_auth_env_paper_trading_detection() {
        let paper_config = create_test_paper_config();
        assert!(paper_config.is_paper_trading());
        assert!(!paper_config.is_live_trading());
    }

    #[test]
    fn test_alpaca_auth_env_live_trading_detection() {
        let live_config = create_test_live_config();
        assert!(!live_config.is_paper_trading());
        assert!(live_config.is_live_trading());
    }

    #[test]
    fn test_alpaca_auth_env_custom_paper_url() {
        let custom_paper = AlpacaAuthEnv {
            api_key_id: "test_key".to_string(),
            api_secret_key: "test_secret".to_string(),
            base_url: "https://custom-paper-api.example.com".to_string(),
        };
        assert!(!custom_paper.is_paper_trading());
        assert!(!custom_paper.is_live_trading());
    }

    #[test]
    fn test_alpaca_client_new_valid_config() {
        let config = create_test_paper_config();
        let result = AlpacaClient::new(&config);

        // Should succeed with valid configuration
        let client =
            result.expect("Expected successful AlpacaClient creation with valid paper config");
        assert!(client.is_paper_trading());
        assert!(!client.is_live_trading());
    }

    #[test]
    fn test_alpaca_client_new_live_config() {
        let config = create_test_live_config();
        let result = AlpacaClient::new(&config);

        // Should succeed with valid configuration
        let client =
            result.expect("Expected successful AlpacaClient creation with valid live config");
        assert!(!client.is_paper_trading());
        assert!(client.is_live_trading());
    }

    #[test]
    fn test_alpaca_client_new_invalid_url() {
        let invalid_config = AlpacaAuthEnv {
            api_key_id: "test_key_id".to_string(),
            api_secret_key: "test_secret_key".to_string(),
            base_url: "not_a_valid_url".to_string(),
        };

        let result = AlpacaClient::new(&invalid_config);

        // Should fail with invalid URL
        assert!(result.is_err());
    }

    #[test]
    fn test_alpaca_client_new_empty_credentials() {
        let empty_config = AlpacaAuthEnv {
            api_key_id: "".to_string(),
            api_secret_key: "".to_string(),
            base_url: "https://paper-api.alpaca.markets".to_string(),
        };

        let result = AlpacaClient::new(&empty_config);

        // apca library accepts empty credentials at creation time
        // validation happens during actual API calls
        let client =
            result.expect("Expected successful AlpacaClient creation with empty credentials");
        assert!(client.is_paper_trading());
    }

    #[test]
    fn test_alpaca_client_paper_vs_live_state_consistency() {
        let paper_config = create_test_paper_config();
        let live_config = create_test_live_config();

        let paper_client =
            AlpacaClient::new(&paper_config).expect("Expected successful paper client creation");
        let live_client =
            AlpacaClient::new(&live_config).expect("Expected successful live client creation");

        // Paper client should detect paper trading correctly
        assert!(paper_client.is_paper_trading());
        assert!(!paper_client.is_live_trading());

        // Live client should detect live trading correctly
        assert!(!live_client.is_paper_trading());
        assert!(live_client.is_live_trading());
    }
}
