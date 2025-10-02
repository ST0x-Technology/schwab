use alloy::primitives::{B256, fixed_bytes};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct FeedIdCache {
    cache: Arc<RwLock<HashMap<String, B256>>>,
}

impl FeedIdCache {
    pub fn new() -> Self {
        let initial = HashMap::from([
            (
                "AAPL".to_string(),
                fixed_bytes!("49f6b65cb1de6b10eaf75e7c03ca029c306d0357e91b5311b175084a5ad55688"),
            ),
            (
                "TSLA".to_string(),
                fixed_bytes!("16dad506d7db8da01c87581c87ca897a012a153557d4d578c3b9c9e1bc0632f1"),
            ),
        ]);

        Self {
            cache: Arc::new(RwLock::new(initial)),
        }
    }

    pub async fn get(&self, symbol: &str) -> Option<B256> {
        self.cache.read().await.get(symbol).copied()
    }

    pub async fn insert(&self, symbol: String, feed_id: B256) {
        let mut cache = self.cache.write().await;
        cache.insert(symbol, feed_id);
    }
}

impl Default for FeedIdCache {
    fn default() -> Self {
        Self::new()
    }
}
