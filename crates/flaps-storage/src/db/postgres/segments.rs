//! PostgreSQL segment repository implementation.

use chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres, Row};
use uuid::Uuid;

use flaps_core::{ProjectId, Segment, SegmentId, UserId};

use crate::error::{StorageError, StorageResult};
use crate::traits::SegmentRepository;

/// PostgreSQL implementation of the segment repository.
#[derive(Debug, Clone)]
pub struct PostgresSegmentRepository {
    pool: Pool<Postgres>,
}

impl PostgresSegmentRepository {
    /// Creates a new PostgreSQL segment repository.
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

impl SegmentRepository for PostgresSegmentRepository {
    async fn get_by_id(&self, id: SegmentId) -> StorageResult<Option<Segment>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, included_users, excluded_users,
                   created_at, updated_at, created_by
            FROM segments
            WHERE id = $1
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
            WHERE project_id = $1 AND key = $2
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
            WHERE project_id = $1
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
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
            SET key = $2, name = $3, description = $4, included_users = $5,
                excluded_users = $6, updated_at = $7
            WHERE id = $1
            "#,
        )
        .bind(segment.id.0.to_string())
        .bind(&segment.key)
        .bind(&segment.name)
        .bind(&segment.description)
        .bind(included_json)
        .bind(excluded_json)
        .bind(Utc::now())
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
        let result = sqlx::query("DELETE FROM segments WHERE id = $1")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found("Segment", "id", id.0.to_string()));
        }

        Ok(())
    }
}

fn row_to_segment(row: &sqlx::postgres::PgRow) -> StorageResult<Segment> {
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
        rules: Vec::new(), // Rules loaded separately from segment_rules/segment_conditions tables
        included_users,
        excluded_users,
        created_at,
        updated_at,
        created_by: UserId::new(created_by),
    })
}
