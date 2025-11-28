//! SQLite segment repository implementation.

use chrono::{DateTime, Utc};
use sqlx::{Pool, Row, Sqlite};
use uuid::Uuid;

use flaps_core::{ProjectId, Segment, SegmentId, UserId};

use crate::error::{StorageError, StorageResult};
use crate::traits::SegmentRepository;

/// SQLite implementation of the segment repository.
#[derive(Debug, Clone)]
pub struct SqliteSegmentRepository {
    pool: Pool<Sqlite>,
}

impl SqliteSegmentRepository {
    /// Creates a new SQLite segment repository.
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }
}

impl SegmentRepository for SqliteSegmentRepository {
    async fn get_by_id(&self, id: SegmentId) -> StorageResult<Option<Segment>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, included_users, excluded_users,
                   created_at, updated_at, created_by
            FROM segments
            WHERE id = ?
            "#,
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_segment(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_by_key(&self, project_id: ProjectId, key: &str) -> StorageResult<Option<Segment>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, included_users, excluded_users,
                   created_at, updated_at, created_by
            FROM segments
            WHERE project_id = ? AND key = ?
            "#,
        )
        .bind(project_id.0.to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_segment(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_by_project(&self, project_id: ProjectId) -> StorageResult<Vec<Segment>> {
        let rows = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, included_users, excluded_users,
                   created_at, updated_at, created_by
            FROM segments
            WHERE project_id = ?
            ORDER BY name ASC
            "#,
        )
        .bind(project_id.0.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_segment).collect()
    }

    async fn create(&self, segment: &Segment) -> StorageResult<()> {
        let included_json = serde_json::to_string(&segment.included_users)?;
        let excluded_json = serde_json::to_string(&segment.excluded_users)?;

        let result = sqlx::query(
            r#"
            INSERT INTO segments (id, project_id, key, name, description, included_users,
                                  excluded_users, created_at, updated_at, created_by)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(segment.id.0.to_string())
        .bind(segment.project_id.0.to_string())
        .bind(&segment.key)
        .bind(&segment.name)
        .bind(&segment.description)
        .bind(included_json)
        .bind(excluded_json)
        .bind(segment.created_at)
        .bind(segment.updated_at)
        .bind(&segment.created_by.0)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(StorageError::duplicate("Segment", "key", &segment.key))
            },
            Err(e) => Err(e.into()),
        }
    }

    async fn update(&self, segment: &Segment) -> StorageResult<()> {
        let included_json = serde_json::to_string(&segment.included_users)?;
        let excluded_json = serde_json::to_string(&segment.excluded_users)?;

        let result = sqlx::query(
            r#"
            UPDATE segments
            SET key = ?, name = ?, description = ?, included_users = ?,
                excluded_users = ?, updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&segment.key)
        .bind(&segment.name)
        .bind(&segment.description)
        .bind(included_json)
        .bind(excluded_json)
        .bind(Utc::now())
        .bind(segment.id.0.to_string())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found(
                "Segment",
                "id",
                segment.id.0.to_string(),
            ));
        }

        Ok(())
    }

    async fn delete(&self, id: SegmentId) -> StorageResult<()> {
        let result = sqlx::query("DELETE FROM segments WHERE id = ?")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found("Segment", "id", id.0.to_string()));
        }

        Ok(())
    }
}

fn row_to_segment(row: &sqlx::sqlite::SqliteRow) -> StorageResult<Segment> {
    let id: String = row.try_get("id")?;
    let project_id: String = row.try_get("project_id")?;
    let key: String = row.try_get("key")?;
    let name: String = row.try_get("name")?;
    let description: Option<String> = row.try_get("description")?;
    let included_json: Option<String> = row.try_get("included_users")?;
    let excluded_json: Option<String> = row.try_get("excluded_users")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let created_by: String = row.try_get("created_by")?;

    let included_users: Vec<String> = included_json
        .map(|j| serde_json::from_str(&j))
        .transpose()?
        .unwrap_or_default();

    let excluded_users: Vec<String> = excluded_json
        .map(|j| serde_json::from_str(&j))
        .transpose()?
        .unwrap_or_default();

    Ok(Segment {
        id: SegmentId::from_uuid(Uuid::parse_str(&id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        project_id: ProjectId::from_uuid(Uuid::parse_str(&project_id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        key,
        name,
        description,
        rules: Vec::new(),
        included_users,
        excluded_users,
        created_at,
        updated_at,
        created_by: UserId::new(created_by),
    })
}
