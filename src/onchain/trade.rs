use alloy::primitives::B256;
use sqlx::SqlitePool;

use crate::error::OnChainError;

#[derive(Debug, Clone, PartialEq)]
pub struct OnchainTrade {
    pub id: Option<i64>,
    pub tx_hash: B256,
    pub log_index: u64,
    pub symbol: String,
    pub amount: f64,
    pub price_usdc: f64,
    pub created_at: Option<String>,
}

impl OnchainTrade {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        let tx_hash_str = self.tx_hash.to_string();
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = self.log_index as i64;

        let result = sqlx::query!(
            r#"
            INSERT INTO onchain_trades (tx_hash, log_index, symbol, amount, price_usdc)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            tx_hash_str,
            log_index_i64,
            self.symbol,
            self.amount,
            self.price_usdc
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn find_by_tx_hash_and_log_index(
        pool: &SqlitePool,
        tx_hash: B256,
        log_index: u64,
    ) -> Result<Self, OnChainError> {
        let tx_hash_str = tx_hash.to_string();
        #[allow(clippy::cast_possible_wrap)]
        let log_index_i64 = log_index as i64;
        let row = sqlx::query!(
            "SELECT * FROM onchain_trades WHERE tx_hash = ?1 AND log_index = ?2",
            tx_hash_str,
            log_index_i64
        )
        .fetch_one(pool)
        .await?;

        let tx_hash = row.tx_hash.parse().map_err(|_| {
            OnChainError::InvalidSchwabInstruction(format!(
                "Invalid tx_hash format: {}",
                row.tx_hash
            ))
        })?;

        Ok(Self {
            id: Some(row.id),
            tx_hash,
            #[allow(clippy::cast_sign_loss)]
            log_index: row.log_index as u64,
            symbol: row.symbol,
            amount: row.amount,
            price_usdc: row.price_usdc,
            created_at: row.created_at.map(|dt| dt.to_string()),
        })
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM onchain_trades")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::fixed_bytes;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_onchain_trade_save_within_transaction_and_find() {
        let pool = setup_test_db().await;

        let trade = OnchainTrade {
            id: None,
            tx_hash: fixed_bytes!(
                "0xbeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            ),
            log_index: 42,
            symbol: "AAPLs1".to_string(),
            amount: 10.0,
            price_usdc: 150.25,
            created_at: None,
        };

        let mut sql_tx = pool.begin().await.unwrap();
        let id = trade.save_within_transaction(&mut sql_tx).await.unwrap();
        sql_tx.commit().await.unwrap();
        assert!(id > 0);

        let found =
            OnchainTrade::find_by_tx_hash_and_log_index(&pool, trade.tx_hash, trade.log_index)
                .await
                .unwrap();

        assert_eq!(found.tx_hash, trade.tx_hash);
        assert_eq!(found.log_index, trade.log_index);
        assert_eq!(found.symbol, trade.symbol);
        assert!((found.amount - trade.amount).abs() < f64::EPSILON);
        assert!((found.price_usdc - trade.price_usdc).abs() < f64::EPSILON);
        assert!(found.id.is_some());
        assert!(found.created_at.is_some());
    }
}
