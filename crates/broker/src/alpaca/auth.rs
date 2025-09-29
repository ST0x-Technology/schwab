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
        self.base_url.contains("paper-api")
    }

    /// Returns true if this configuration is for live trading
    pub fn is_live_trading(&self) -> bool {
        !self.is_paper_trading()
    }
}
