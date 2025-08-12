use sqlx::SqlitePool;

use crate::onchain::{TradeConversionError, TradeStatus};
use crate::schwab::SchwabInstruction;

macro_rules! convert_rows_to_executions {
    ($rows:expr) => {{
        let mut executions = Vec::new();

        for row in $rows {
            let direction = row
                .direction
                .parse()
                .map_err(TradeConversionError::InvalidSchwabInstruction)?;

            let status = row
                .status
                .parse()
                .map_err(TradeConversionError::InvalidTradeStatus)?;

            executions.push(SchwabExecution {
                id: Some(row.id),
                symbol: row.symbol,
                #[allow(clippy::cast_sign_loss)]
                shares: row.shares as u64,
                direction,
                order_id: row.order_id,
                #[allow(clippy::cast_sign_loss)]
                price_cents: row.price_cents.map(|p| p as u64),
                status,
                executed_at: row.executed_at.map(|dt| dt.to_string()),
            });
        }

        Ok(executions)
    }};
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchwabExecution {
    pub id: Option<i64>,
    pub symbol: String,
    pub shares: u64,
    pub direction: SchwabInstruction,
    pub order_id: Option<String>,
    pub price_cents: Option<u64>,
    pub status: TradeStatus,
    pub executed_at: Option<String>,
}

impl SchwabExecution {
    pub async fn save_within_transaction(
        &self,
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    ) -> Result<i64, sqlx::Error> {
        #[allow(clippy::cast_possible_wrap)]
        let shares_i64 = self.shares as i64;
        let direction_str = self.direction.as_str();
        #[allow(clippy::cast_possible_wrap)]
        let price_cents_i64 = self.price_cents.map(|p| p as i64);
        let status_str = self.status.as_str();

        let result = sqlx::query!(
            r#"
            INSERT INTO schwab_executions (symbol, shares, direction, order_id, price_cents, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            self.symbol,
            shares_i64,
            direction_str,
            self.order_id,
            price_cents_i64,
            status_str
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(result.last_insert_rowid())
    }


    pub async fn find_by_symbol_and_status(
        pool: &SqlitePool,
        symbol: &str,
        status: TradeStatus,
    ) -> Result<Vec<Self>, TradeConversionError> {
        let status_str = status.as_str();

        if symbol.is_empty() {
            let rows = sqlx::query!(
                "SELECT * FROM schwab_executions WHERE status = ?1 ORDER BY executed_at ASC",
                status_str
            )
            .fetch_all(pool)
            .await?;

            convert_rows_to_executions!(rows)
        } else {
            let rows = sqlx::query!(
                "SELECT * FROM schwab_executions WHERE symbol = ?1 AND status = ?2 ORDER BY executed_at ASC",
                symbol,
                status_str
            )
            .fetch_all(pool)
            .await?;

            convert_rows_to_executions!(rows)
        }
    }


    pub async fn update_status_within_transaction(
        sql_tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        execution_id: i64,
        new_status: TradeStatus,
        order_id: Option<String>,
        price_cents: Option<u64>,
    ) -> Result<(), sqlx::Error> {
        let status_str = new_status.as_str();
        #[allow(clippy::cast_possible_wrap)]
        let price_cents_i64 = price_cents.map(|p| p as i64);
        sqlx::query!(
            "UPDATE schwab_executions SET status = ?1, order_id = ?2, price_cents = ?3 WHERE id = ?4",
            status_str,
            order_id,
            price_cents_i64,
            execution_id
        )
        .execute(&mut **sql_tx)
        .await?;

        Ok(())
    }

    #[cfg(test)]
    pub async fn db_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
        let row = sqlx::query!("SELECT COUNT(*) as count FROM schwab_executions")
            .fetch_one(pool)
            .await?;
        Ok(row.count)
    }
}

