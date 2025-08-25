use crate::bindings::IOrderBookV4::{EvaluableV3, IO, OrderV3};
use alloy::primitives::{LogData, U256, address, bytes, fixed_bytes};
use alloy::rpc::types::Log;

/// Returns a test `OrderV3` instance that is shared across multiple
/// unit-tests. The exact values are not important – only that the
/// structure is valid and deterministic.
pub fn get_test_order() -> OrderV3 {
    OrderV3 {
        owner: address!("0x1111111111111111111111111111111111111111"),
        evaluable: EvaluableV3 {
            interpreter: address!("0x2222222222222222222222222222222222222222"),
            store: address!("0x3333333333333333333333333333333333333333"),
            bytecode: bytes!("0x00"),
        },
        nonce: fixed_bytes!("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        validInputs: vec![
            IO {
                token: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                decimals: 6, // USDC-like token
                vaultId: U256::from(0),
            },
            IO {
                token: address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                decimals: 18, // Stock share token
                vaultId: U256::from(0),
            },
        ],
        validOutputs: vec![
            IO {
                token: address!("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                decimals: 6,
                vaultId: U256::from(0),
            },
            IO {
                token: address!("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                decimals: 18,
                vaultId: U256::from(0),
            },
        ],
    }
}

/// Creates a generic `Log` stub with the supplied log index. This helper is
/// useful when the concrete value of most fields is irrelevant for the
/// assertion being performed.
pub fn create_log(log_index: u64) -> Log {
    Log {
        inner: alloy::primitives::Log {
            address: address!("0xfefefefefefefefefefefefefefefefefefefefe"),
            data: LogData::empty(),
        },
        block_hash: None,
        block_number: Some(12345),
        block_timestamp: None,
        transaction_hash: Some(fixed_bytes!(
            "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        )),
        transaction_index: None,
        log_index: Some(log_index),
        removed: false,
    }
}

/// Convenience wrapper that returns the log routinely used by the
/// higher-level tests in `trade::mod` (with log index set to `293`).
pub fn get_test_log() -> Log {
    create_log(293)
}

use sqlx::SqlitePool;

/// Centralized test database setup to eliminate duplication across test files.
/// Creates an in-memory SQLite database with all migrations applied.
pub async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

use crate::onchain::OnchainTrade;
use crate::schwab::TradeStatus;
use crate::schwab::{Direction, execution::SchwabExecution};

/// Builder for creating OnchainTrade test instances with sensible defaults.
/// Reduces duplication in test data setup.
pub struct OnchainTradeBuilder {
    trade: OnchainTrade,
}

impl Default for OnchainTradeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl OnchainTradeBuilder {
    pub fn new() -> Self {
        Self {
            trade: OnchainTrade {
                id: None,
                tx_hash: fixed_bytes!(
                    "0x1111111111111111111111111111111111111111111111111111111111111111"
                ),
                log_index: 1,
                symbol: "AAPLs1".to_string(),
                amount: 1.0,
                direction: Direction::Buy,
                price_usdc: 150.0,
                created_at: None,
            },
        }
    }

    #[must_use]
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.trade.symbol = symbol.into();
        self
    }

    #[must_use]
    pub fn with_amount(mut self, amount: f64) -> Self {
        self.trade.amount = amount;
        self
    }

    #[must_use]
    pub fn with_price(mut self, price: f64) -> Self {
        self.trade.price_usdc = price;
        self
    }

    #[must_use]
    pub fn with_tx_hash(mut self, hash: alloy::primitives::B256) -> Self {
        self.trade.tx_hash = hash;
        self
    }

    #[must_use]
    pub fn with_log_index(mut self, index: u64) -> Self {
        self.trade.log_index = index;
        self
    }

    pub fn build(self) -> OnchainTrade {
        self.trade
    }
}

/// Builder for creating SchwabExecution test instances with sensible defaults.
/// Reduces duplication in test data setup.
pub struct SchwabExecutionBuilder {
    execution: SchwabExecution,
}

impl Default for SchwabExecutionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SchwabExecutionBuilder {
    pub fn new() -> Self {
        Self {
            execution: SchwabExecution {
                id: None,
                symbol: "AAPL".to_string(),
                shares: 100,
                direction: Direction::Buy,
                status: TradeStatus::Pending,
            },
        }
    }

    #[must_use]
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.execution.symbol = symbol.into();
        self
    }

    #[must_use]
    pub fn with_shares(mut self, shares: u64) -> Self {
        self.execution.shares = shares;
        self
    }

    #[must_use]
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.execution.direction = direction;
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: TradeStatus) -> Self {
        self.execution.status = status;
        self
    }

    pub fn build(self) -> SchwabExecution {
        self.execution
    }
}
