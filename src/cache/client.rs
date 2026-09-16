use crate::error::{AppError, AppResult};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::cache::keys;

/// All Redis access goes through here, so the set of key shapes in use is the
/// set of functions in `cache::keys` and nothing else.
#[derive(Clone)]
pub struct Cache {
    conn: ConnectionManager,
}

impl Cache {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let mut conn = self.conn.clone();
        let raw: Option<String> = conn.get(key).await.map_err(AppError::Cache)?;
        Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
    }

    pub async fn set_json<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl_seconds: u64,
    ) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let raw = serde_json::to_string(value)
            .map_err(|e| AppError::Internal(format!("serializing cache value: {e}")))?;
        conn.set_ex(key, raw, ttl_seconds).await.map_err(AppError::Cache)
    }

    pub async fn set_marker(&self, key: &str, ttl_seconds: u64) -> AppResult<()> {
        let mut conn = self.conn.clone();
        conn.set_ex(key, 1u8, ttl_seconds).await.map_err(AppError::Cache)
    }

    /// Invalidate the cached task list for each of these users.
    ///
    /// Callers pass every affected user, which for a reassignment means both the
    /// previous and the new assignee. Dropping the previous one leaves the task
    /// visible in their cached list until the TTL lapses.
    pub async fn invalidate_my_tasks(
        &self,
        user_ids: impl IntoIterator<Item = Uuid>,
    ) -> AppResult<()> {
        let mut conn = self.conn.clone();
        let mut seen = std::collections::HashSet::new();
        let targets: Vec<String> = user_ids
            .into_iter()
            .filter(|id| seen.insert(*id))
            .map(keys::my_tasks)
            .collect();
        if targets.is_empty() {
            return Ok(());
        }
        let _: u64 = conn.del(targets).await.map_err(AppError::Cache)?;
        Ok(())
    }
}
