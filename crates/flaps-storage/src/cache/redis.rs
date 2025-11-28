//! Redis cache implementation for flag configurations.

use redis::{aio::ConnectionManager, AsyncCommands, Client};

use flaps_core::{Flag, ProjectId};

use crate::error::{StorageError, StorageResult};
use crate::traits::FlagCache;

/// Configuration for the Redis cache.
#[derive(Debug, Clone)]
pub struct RedisCacheConfig {
    /// Redis connection URL.
    pub url: String,
    /// Key prefix for all Flaps keys.
    pub key_prefix: String,
    /// Default TTL in seconds.
    pub default_ttl_secs: u64,
}

impl Default for RedisCacheConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            key_prefix: "flaps".to_string(),
            default_ttl_secs: 300, // 5 minutes
        }
    }
}

impl RedisCacheConfig {
    /// Creates a new Redis cache configuration.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }

    /// Sets a custom key prefix.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// Sets the default TTL.
    pub fn with_ttl(mut self, ttl_secs: u64) -> Self {
        self.default_ttl_secs = ttl_secs;
        self
    }
}

/// Redis implementation of the flag cache.
#[derive(Clone)]
pub struct RedisFlagCache {
    conn: ConnectionManager,
    config: RedisCacheConfig,
}

impl std::fmt::Debug for RedisFlagCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisFlagCache")
            .field("config", &self.config)
            .finish()
    }
}

impl RedisFlagCache {
    /// Creates a new Redis flag cache.
    pub async fn new(config: RedisCacheConfig) -> StorageResult<Self> {
        let client = Client::open(config.url.as_str()).map_err(|e| {
            StorageError::Configuration(format!("Failed to create Redis client: {}", e))
        })?;

        let conn = ConnectionManager::new(client).await?;

        Ok(Self { conn, config })
    }

    /// Creates a cache key for flags.
    fn flags_key(&self, project_id: ProjectId, environment: &str) -> String {
        format!(
            "{}:flags:{}:{}",
            self.config.key_prefix, project_id.0, environment
        )
    }

    /// Creates a pattern for invalidating all environments of a project.
    fn project_pattern(&self, project_id: ProjectId) -> String {
        format!("{}:flags:{}:*", self.config.key_prefix, project_id.0)
    }

    /// Checks if Redis is healthy.
    pub async fn is_healthy(&self) -> bool {
        let mut conn = self.conn.clone();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .is_ok()
    }
}

impl FlagCache for RedisFlagCache {
    async fn get(
        &self,
        project_id: ProjectId,
        environment: &str,
    ) -> StorageResult<Option<Vec<Flag>>> {
        let key = self.flags_key(project_id, environment);
        let mut conn = self.conn.clone();

        let data: Option<String> = conn.get(&key).await?;

        match data {
            Some(json) => {
                let flags: Vec<Flag> = serde_json::from_str(&json)?;
                Ok(Some(flags))
            },
            None => Ok(None),
        }
    }

    async fn set(
        &self,
        project_id: ProjectId,
        environment: &str,
        flags: &[Flag],
        ttl_secs: u64,
    ) -> StorageResult<()> {
        let key = self.flags_key(project_id, environment);
        let json = serde_json::to_string(flags)?;
        let mut conn = self.conn.clone();

        conn.set_ex::<_, _, ()>(&key, json, ttl_secs).await?;

        Ok(())
    }

    async fn invalidate(
        &self,
        project_id: ProjectId,
        environment: Option<&str>,
    ) -> StorageResult<()> {
        let mut conn = self.conn.clone();

        match environment {
            Some(env) => {
                // Invalidate specific environment
                let key = self.flags_key(project_id, env);
                conn.del::<_, ()>(&key).await?;
            },
            None => {
                // Invalidate all environments for this project
                let pattern = self.project_pattern(project_id);
                let keys: Vec<String> = redis::cmd("KEYS")
                    .arg(&pattern)
                    .query_async(&mut conn)
                    .await?;

                if !keys.is_empty() {
                    conn.del::<_, ()>(keys).await?;
                }
            },
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RedisCacheConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.key_prefix, "flaps");
        assert_eq!(config.default_ttl_secs, 300);
    }

    #[test]
    fn test_config_builder() {
        let config = RedisCacheConfig::new("redis://localhost:6380")
            .with_prefix("myapp")
            .with_ttl(600);

        assert_eq!(config.url, "redis://localhost:6380");
        assert_eq!(config.key_prefix, "myapp");
        assert_eq!(config.default_ttl_secs, 600);
    }
}
