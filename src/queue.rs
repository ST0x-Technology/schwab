use alloy::primitives::B256;
use alloy::rpc::types::Log;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::str::FromStr;

use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::error::EventQueueError;

/// Union type for all blockchain events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SerializableEvent {
    ClearV2(Box<ClearV2>),
    TakeOrderV2(Box<TakeOrderV2>),
}

/// Trait for events that can be enqueued
pub trait Enqueueable {
    fn to_serializable_event(&self) -> SerializableEvent;
}

impl Enqueueable for ClearV2 {
    fn to_serializable_event(&self) -> SerializableEvent {
        SerializableEvent::ClearV2(Box::new(self.clone()))
    }
}

impl Enqueueable for TakeOrderV2 {
    fn to_serializable_event(&self) -> SerializableEvent {
        SerializableEvent::TakeOrderV2(Box::new(self.clone()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub id: Option<i64>,
    pub tx_hash: B256,
    pub log_index: u64,
    pub block_number: u64,
    pub event: SerializableEvent,
    pub processed: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub processed_at: Option<DateTime<Utc>>,
}

impl QueuedEvent {
    pub fn new(log: &Log, event: SerializableEvent) -> Result<Self, EventQueueError> {
        let tx_hash = log.transaction_hash.ok_or_else(|| {
            EventQueueError::Processing("Log missing transaction hash".to_string())
        })?;

        let log_index = log
            .log_index
            .ok_or_else(|| EventQueueError::Processing("Log missing log index".to_string()))?;

        let block_number = log
            .block_number
            .ok_or_else(|| EventQueueError::Processing("Log missing block number".to_string()))?;

        Ok(Self {
            id: None,
            tx_hash,
            log_index,
            block_number,
            event,
            processed: false,
            created_at: None,
            processed_at: None,
        })
    }
}

pub async fn enqueue_event(
    pool: &SqlitePool,
    log: &Log,
    event: SerializableEvent,
) -> Result<(), EventQueueError> {
    let tx_hash = log
        .transaction_hash
        .ok_or_else(|| EventQueueError::Processing("Log missing transaction hash".to_string()))?;

    let log_index = log
        .log_index
        .ok_or_else(|| EventQueueError::Processing("Log missing log index".to_string()))?;

    let log_index_i64 = i64::try_from(log_index)
        .map_err(|_| EventQueueError::Processing("Log index too large".to_string()))?;

    let block_number = log
        .block_number
        .ok_or_else(|| EventQueueError::Processing("Log missing block number".to_string()))?;

    let block_number_i64 = i64::try_from(block_number)
        .map_err(|_| EventQueueError::Processing("Block number too large".to_string()))?;

    let tx_hash_str = format!("{tx_hash:#x}");
    let event_json = serde_json::to_string(&event)
        .map_err(|e| EventQueueError::Processing(format!("Failed to serialize event: {e}")))?;

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO event_queue 
        (tx_hash, log_index, block_number, event_data, processed)
        VALUES (?, ?, ?, ?, 0)
        "#,
        tx_hash_str,
        log_index_i64,
        block_number_i64,
        event_json
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Gets the next unprocessed event from the queue, ordered by block number then log index for deterministic processing
pub async fn get_next_unprocessed_event(
    pool: &SqlitePool,
) -> Result<Option<QueuedEvent>, EventQueueError> {
    let row = sqlx::query!(
        r#"
        SELECT id, tx_hash, log_index, block_number, event_data, processed, created_at, processed_at
        FROM event_queue
        WHERE processed = 0
        ORDER BY block_number ASC, log_index ASC
        LIMIT 1
        "#
    )
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let tx_hash = B256::from_str(&row.tx_hash)
        .map_err(|e| EventQueueError::Processing(format!("Invalid tx_hash format: {e}")))?;

    let event: SerializableEvent = serde_json::from_str(&row.event_data)
        .map_err(|e| EventQueueError::Processing(format!("Failed to deserialize event: {e}")))?;

    Ok(Some(QueuedEvent {
        id: Some(row.id),
        tx_hash,
        log_index: row
            .log_index
            .try_into()
            .map_err(|_| EventQueueError::Processing("Log index conversion failed".to_string()))?,
        block_number: row.block_number.try_into().map_err(|_| {
            EventQueueError::Processing("Block number conversion failed".to_string())
        })?,
        event,
        processed: row.processed,
        created_at: Some(row.created_at.and_utc()),
        processed_at: row.processed_at.map(|dt| dt.and_utc()),
    }))
}

/// Marks an event as processed in the queue
pub async fn mark_event_processed(pool: &SqlitePool, event_id: i64) -> Result<(), EventQueueError> {
    sqlx::query!(
        r#"
        UPDATE event_queue 
        SET processed = 1, processed_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
        event_id
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Gets count of unprocessed events in the queue
pub async fn get_unprocessed_count(pool: &SqlitePool) -> Result<i64, EventQueueError> {
    let row = sqlx::query!("SELECT COUNT(*) as count FROM event_queue WHERE processed = 0")
        .fetch_one(pool)
        .await?;

    Ok(row.count)
}

/// Gets all unprocessed events from the queue in deterministic order
pub async fn get_all_unprocessed_events(
    pool: &SqlitePool,
) -> Result<Vec<QueuedEvent>, EventQueueError> {
    let rows = sqlx::query!(
        r#"
        SELECT id, tx_hash, log_index, block_number, event_data, processed, created_at, processed_at
        FROM event_queue
        WHERE processed = 0
        ORDER BY block_number ASC, log_index ASC
        "#
    )
    .fetch_all(pool)
    .await?;

    let mut events = Vec::new();
    for row in rows {
        let tx_hash = B256::from_str(&row.tx_hash)
            .map_err(|e| EventQueueError::Processing(format!("Invalid tx_hash format: {e}")))?;

        let event: SerializableEvent = serde_json::from_str(&row.event_data).map_err(|e| {
            EventQueueError::Processing(format!("Failed to deserialize event: {e}"))
        })?;

        events.push(QueuedEvent {
            id: Some(row.id),
            tx_hash,
            log_index: row.log_index.try_into().map_err(|_| {
                EventQueueError::Processing("Log index conversion failed".to_string())
            })?,
            block_number: row.block_number.try_into().map_err(|_| {
                EventQueueError::Processing("Block number conversion failed".to_string())
            })?,
            event,
            processed: row.processed,
            created_at: Some(row.created_at.and_utc()),
            processed_at: row.processed_at.map(|dt| dt.and_utc()),
        });
    }

    Ok(events)
}

/// Generic function to enqueue any event that implements Enqueueable
#[allow(clippy::future_not_send)]
pub async fn enqueue<E: Enqueueable>(
    pool: &SqlitePool,
    event: &E,
    log: &Log,
) -> Result<(), EventQueueError> {
    let serializable_event = event.to_serializable_event();
    enqueue_event(pool, log, serializable_event).await
}

/// Gets the event from a queued event (no deserialization needed since it's already typed)
pub const fn get_event(queued_event: &QueuedEvent) -> &SerializableEvent {
    &queued_event.event
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::IOrderBookV4::{
        ClearConfig, ClearV2, OrderV3, TakeOrderConfigV3, TakeOrderV2,
    };
    use crate::test_utils::setup_test_db;
    use alloy::primitives::{LogData, Uint, address, b256};

    #[tokio::test]
    async fn test_enqueue_and_process_event() {
        let pool = setup_test_db().await;

        let log = Log {
            inner: alloy::primitives::Log {
                address: address!("1234567890123456789012345678901234567890"),
                data: LogData::default(),
            },
            block_hash: Some(b256!(
                "1111111111111111111111111111111111111111111111111111111111111111"
            )),
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(b256!(
                "2222222222222222222222222222222222222222222222222222222222222222"
            )),
            transaction_index: Some(1),
            log_index: Some(5),
            removed: false,
        };

        // Create a test event
        let test_event = SerializableEvent::ClearV2(Box::new(ClearV2 {
            sender: log.inner.address,
            alice: OrderV3::default(),
            bob: OrderV3::default(),
            clearConfig: ClearConfig::default(),
        }));

        // Enqueue event
        enqueue_event(&pool, &log, test_event.clone())
            .await
            .unwrap();

        // Check unprocessed count
        let count = get_unprocessed_count(&pool).await.unwrap();
        assert_eq!(count, 1);

        // Get next unprocessed event
        let queued_event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
        assert_eq!(queued_event.tx_hash, log.transaction_hash.unwrap());
        assert_eq!(queued_event.log_index, 5);
        assert_eq!(queued_event.block_number, 100);
        assert!(matches!(queued_event.event, SerializableEvent::ClearV2(_)));
        assert!(!queued_event.processed);

        // Mark as processed
        mark_event_processed(&pool, queued_event.id.unwrap())
            .await
            .unwrap();

        // Check unprocessed count is now 0
        let count = get_unprocessed_count(&pool).await.unwrap();
        assert_eq!(count, 0);

        // Should return None for next unprocessed
        let next_event = get_next_unprocessed_event(&pool).await.unwrap();
        assert!(next_event.is_none());
    }

    #[tokio::test]
    async fn test_duplicate_event_handling() {
        let pool = setup_test_db().await;

        let log = Log {
            inner: alloy::primitives::Log {
                address: address!("1234567890123456789012345678901234567890"),
                data: LogData::default(),
            },
            block_hash: Some(b256!(
                "1111111111111111111111111111111111111111111111111111111111111111"
            )),
            block_number: Some(100),
            block_timestamp: None,
            transaction_hash: Some(b256!(
                "2222222222222222222222222222222222222222222222222222222222222222"
            )),
            transaction_index: Some(1),
            log_index: Some(5),
            removed: false,
        };

        // Create a test event
        let test_event = SerializableEvent::TakeOrderV2(Box::new(TakeOrderV2 {
            sender: log.inner.address,
            config: TakeOrderConfigV3::default(),
            input: Uint::default(),
            output: Uint::default(),
        }));

        // Enqueue same event twice
        enqueue_event(&pool, &log, test_event.clone())
            .await
            .unwrap();
        enqueue_event(&pool, &log, test_event.clone())
            .await
            .unwrap();

        // Should only have one event due to unique constraint
        let count = get_unprocessed_count(&pool).await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_event_ordering() {
        let pool = setup_test_db().await;

        // Create multiple events with different timestamps
        for i in 0..3 {
            let log = Log {
                inner: alloy::primitives::Log {
                    address: address!("1234567890123456789012345678901234567890"),
                    data: LogData::default(),
                },
                block_hash: Some(b256!(
                    "1111111111111111111111111111111111111111111111111111111111111111"
                )),
                block_number: Some(100 + i),
                block_timestamp: None,
                transaction_hash: Some(B256::from([u8::try_from(i).unwrap_or(0); 32])),
                transaction_index: Some(1),
                log_index: Some(i),
                removed: false,
            };

            let test_event = SerializableEvent::ClearV2(Box::new(ClearV2 {
                sender: log.inner.address,
                alice: OrderV3::default(),
                bob: OrderV3::default(),
                clearConfig: ClearConfig::default(),
            }));
            enqueue_event(&pool, &log, test_event).await.unwrap();
        }

        // Events should be returned in creation order
        for i in 0..3 {
            let event = get_next_unprocessed_event(&pool).await.unwrap().unwrap();
            assert_eq!(event.log_index, i);
            mark_event_processed(&pool, event.id.unwrap())
                .await
                .unwrap();
        }

        let count = get_unprocessed_count(&pool).await.unwrap();
        assert_eq!(count, 0);
    }
}
