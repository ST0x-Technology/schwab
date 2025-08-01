use alloy::hex;
use alloy::primitives::B256;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::trade::{PartialArbTrade, SchwabInstruction, TradeConversionError, TradeStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct ArbTrade {
    pub id: Option<i64>,
    pub tx_hash: B256,
    pub log_index: u64,

    pub onchain_input_symbol: String,
    pub onchain_input_amount: f64,
    pub onchain_output_symbol: String,
    pub onchain_output_amount: f64,
    pub onchain_io_ratio: f64,
    pub onchain_price_per_share_cents: f64,

    pub schwab_ticker: String,
    pub schwab_instruction: SchwabInstruction,
    pub schwab_quantity: f64,
    pub schwab_price_per_share_cents: Option<i64>,

    pub status: TradeStatus,
    pub schwab_order_id: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ArbTrade {
    pub fn from_partial_trade(partial_trade: PartialArbTrade) -> Self {
        Self {
            id: None,
            tx_hash: partial_trade.tx_hash,
            log_index: partial_trade.log_index,
            onchain_input_symbol: partial_trade.onchain_input_symbol,
            onchain_input_amount: partial_trade.onchain_input_amount,
            onchain_output_symbol: partial_trade.onchain_output_symbol,
            onchain_output_amount: partial_trade.onchain_output_amount,
            onchain_io_ratio: partial_trade.onchain_io_ratio,
            schwab_ticker: partial_trade.schwab_ticker,
            schwab_instruction: partial_trade.schwab_instruction,
            schwab_quantity: partial_trade.schwab_quantity,
            onchain_price_per_share_cents: partial_trade.onchain_price_per_share_cents,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            created_at: None,
            completed_at: None,
        }
    }

    pub async fn try_save_to_db(&self, pool: &SqlitePool) -> Result<bool, TradeConversionError> {
        let tx_hash_hex = hex::encode_prefixed(self.tx_hash.as_slice());
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = self.log_index as i64;
        let schwab_instruction_str = match self.schwab_instruction {
            SchwabInstruction::Buy => "BUY",
            SchwabInstruction::Sell => "SELL",
        };
        let status_str = self.status.as_str();

        let result = sqlx::query!(
            r#"
            INSERT INTO trades (
                tx_hash, log_index,
                onchain_input_symbol, onchain_input_amount,
                onchain_output_symbol, onchain_output_amount, onchain_io_ratio,
                schwab_ticker, schwab_instruction, schwab_quantity, onchain_price_per_share_cents,
                schwab_price_per_share_cents, status, schwab_order_id, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))
            ON CONFLICT(tx_hash, log_index) DO NOTHING
            "#,
            tx_hash_hex,
            log_index_i64,
            self.onchain_input_symbol,
            self.onchain_input_amount,
            self.onchain_output_symbol,
            self.onchain_output_amount,
            self.onchain_io_ratio,
            self.schwab_ticker,
            schwab_instruction_str,
            self.schwab_quantity,
            self.onchain_price_per_share_cents,
            self.schwab_price_per_share_cents,
            status_str,
            self.schwab_order_id,
        )
        .execute(pool)
        .await?;

        // If rows_affected is 0, the trade already existed and was not inserted
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_status(
        pool: &SqlitePool,
        tx_hash: B256,
        log_index: u64,
        status: TradeStatus,
    ) -> Result<(), TradeConversionError> {
        let tx_hash_hex = hex::encode_prefixed(tx_hash.as_slice());
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = log_index as i64;
        let status_str = status.as_str();

        sqlx::query!(
            r#"
            UPDATE trades 
            SET status = ?, completed_at = datetime('now') 
            WHERE tx_hash = ? AND log_index = ?
            "#,
            status_str,
            tx_hash_hex,
            log_index_i64
        )
        .execute(pool)
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::fixed_bytes;
    use sqlx::SqlitePool;

    use super::*;

    #[tokio::test]
    async fn test_try_save_to_db_success() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let trade = ArbTrade {
            tx_hash: fixed_bytes!(
                "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd"
            ),
            log_index: 123,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 1000.0,
            onchain_output_symbol: "AAPLs1".to_string(),
            onchain_output_amount: 5.0,
            onchain_io_ratio: 200.0,
            schwab_ticker: "AAPL".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 5.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            id: None,
            created_at: None,
            completed_at: None,
        };

        let was_inserted = trade.try_save_to_db(&pool).await.unwrap();
        assert!(was_inserted);

        let saved_trade = sqlx::query!(
            "SELECT * FROM trades WHERE tx_hash = ? AND log_index = ?",
            "0xabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd",
            123_i64
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(saved_trade.onchain_input_symbol.unwrap(), "USDC");
        assert!((saved_trade.onchain_input_amount.unwrap() - 1000.0).abs() < f64::EPSILON);
        assert_eq!(saved_trade.onchain_output_symbol.unwrap(), "AAPLs1");
        assert!((saved_trade.onchain_output_amount.unwrap() - 5.0).abs() < f64::EPSILON);
        assert!((saved_trade.onchain_io_ratio.unwrap() - 200.0).abs() < f64::EPSILON);
        assert_eq!(saved_trade.schwab_ticker.unwrap(), "AAPL");
        assert_eq!(saved_trade.schwab_instruction.unwrap(), "BUY");
        assert!((saved_trade.schwab_quantity.unwrap() - 5.0).abs() < f64::EPSILON);
        assert!(
            (saved_trade.onchain_price_per_share_cents.unwrap() - 20000.0).abs() < f64::EPSILON
        );
        assert!(saved_trade.schwab_price_per_share_cents.is_none());
        assert_eq!(saved_trade.status.unwrap(), "PENDING");
    }

    #[tokio::test]
    async fn test_try_save_to_db_duplicate_ignored() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let trade = ArbTrade {
            tx_hash: fixed_bytes!(
                "0x1111111111111111111111111111111111111111111111111111111111111111"
            ),
            log_index: 456,
            onchain_input_symbol: "BARs1".to_string(),
            onchain_input_amount: 10.0,
            onchain_output_symbol: "USDC".to_string(),
            onchain_output_amount: 2000.0,
            onchain_io_ratio: 0.005,
            schwab_ticker: "BAR".to_string(),
            schwab_instruction: SchwabInstruction::Sell,
            schwab_quantity: 10.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_price_per_share_cents: None,
            status: TradeStatus::Pending,
            schwab_order_id: None,
            id: None,
            created_at: None,
            completed_at: None,
        };

        let first_insert = trade.try_save_to_db(&pool).await.unwrap();
        assert!(first_insert);

        let duplicate_insert = trade.try_save_to_db(&pool).await.unwrap();
        assert!(!duplicate_insert);
    }

    #[tokio::test]
    async fn test_from_partial_trade() {
        let partial_trade = PartialArbTrade {
            tx_hash: fixed_bytes!(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            log_index: 100,
            onchain_input_symbol: "USDC".to_string(),
            onchain_input_amount: 500.0,
            onchain_output_symbol: "GOOGs1".to_string(),
            onchain_output_amount: 2.5,
            onchain_io_ratio: 200.0,
            onchain_price_per_share_cents: 20000.0,
            schwab_ticker: "GOOG".to_string(),
            schwab_instruction: SchwabInstruction::Buy,
            schwab_quantity: 2.5,
        };

        let arb_trade = ArbTrade::from_partial_trade(partial_trade.clone());

        assert_eq!(arb_trade.tx_hash, partial_trade.tx_hash);
        assert_eq!(arb_trade.log_index, partial_trade.log_index);
        assert_eq!(
            arb_trade.onchain_input_symbol,
            partial_trade.onchain_input_symbol
        );
        assert!(
            (arb_trade.onchain_input_amount - partial_trade.onchain_input_amount).abs()
                < f64::EPSILON
        );
        assert_eq!(
            arb_trade.onchain_output_symbol,
            partial_trade.onchain_output_symbol
        );
        assert!(
            (arb_trade.onchain_output_amount - partial_trade.onchain_output_amount).abs()
                < f64::EPSILON
        );
        assert!((arb_trade.onchain_io_ratio - partial_trade.onchain_io_ratio).abs() < f64::EPSILON);
        assert!(
            (arb_trade.onchain_price_per_share_cents - partial_trade.onchain_price_per_share_cents)
                .abs()
                < f64::EPSILON
        );
        assert_eq!(arb_trade.schwab_ticker, partial_trade.schwab_ticker);
        assert_eq!(
            arb_trade.schwab_instruction,
            partial_trade.schwab_instruction
        );
        assert!((arb_trade.schwab_quantity - partial_trade.schwab_quantity).abs() < f64::EPSILON);
        assert_eq!(arb_trade.status, TradeStatus::Pending);
        assert_eq!(arb_trade.schwab_order_id, None);
    }
}
