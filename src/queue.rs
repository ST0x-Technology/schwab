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
    ClearV2(ClearV2),
    TakeOrderV2(TakeOrderV2),
}

/// Trait for events that can be enqueued
pub trait Enqueueable {
    fn to_serializable_event(&self) -> SerializableEvent;
}

impl Enqueueable for ClearV2 {
    fn to_serializable_event(&self) -> SerializableEvent {
        SerializableEvent::ClearV2(self.clone())
    }
}

impl Enqueueable for TakeOrderV2 {
    fn to_serializable_event(&self) -> SerializableEvent {
        SerializableEvent::TakeOrderV2(self.clone())
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
            .ok_or_else(|| EventQueueError::Processing("Log missing log index".to_string()))?
            as u64;

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
        .ok_or_else(|| EventQueueError::Processing("Log missing log index".to_string()))?
        as i64;

    let block_number = log
        .block_number
        .ok_or_else(|| EventQueueError::Processing("Log missing block number".to_string()))?
        as i64;

    let tx_hash_str = format!("{:#x}", tx_hash);
    let event_json = serde_json::to_string(&event)
        .map_err(|e| EventQueueError::Processing(format!("Failed to serialize event: {e}")))?;

    sqlx::query!(
        r#"
        INSERT OR IGNORE INTO event_queue 
        (tx_hash, log_index, block_number, event_data, processed)
        VALUES (?, ?, ?, ?, 0)
        "#,
        tx_hash_str,
        log_index,
        block_number,
        event_json
    )
    .execute(pool)
    .await?;

    Ok(())
}

