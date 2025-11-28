//! PostgreSQL flag repository implementation.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres, Row};
use uuid::Uuid;

use flaps_core::{Flag, FlagId, FlagKey, FlagType, ProjectId, UserId};

use crate::error::{StorageError, StorageResult};
use crate::traits::FlagRepository;

/// PostgreSQL implementation of the flag repository.
#[derive(Debug, Clone)]
pub struct PostgresFlagRepository {
    pool: Pool<Postgres>,
}

impl PostgresFlagRepository {
    /// Creates a new PostgreSQL flag repository.
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

impl FlagRepository for PostgresFlagRepository {
    async fn get_by_id(&self, id: FlagId) -> StorageResult<Option<Flag>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, flag_type, variants, tags,
                   created_at, updated_at, created_by
            FROM flags
            WHERE id = $1
            "#,
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_flag(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_by_key(
        &self,
        project_id: ProjectId,
        key: &FlagKey,
    ) -> StorageResult<Option<Flag>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, flag_type, variants, tags,
                   created_at, updated_at, created_by
            FROM flags
            WHERE project_id = $1 AND key = $2
            "#,
        )
        .bind(project_id.0.to_string())
        .bind(key.as_str())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_flag(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_by_project(&self, project_id: ProjectId) -> StorageResult<Vec<Flag>> {
        let rows = sqlx::query(
            r#"
            SELECT id, project_id, key, name, description, flag_type, variants, tags,
                   created_at, updated_at, created_by
            FROM flags
            WHERE project_id = $1
            ORDER BY name ASC
            "#,
        )
        .bind(project_id.0.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_flag).collect()
    }

    async fn create(&self, flag: &Flag) -> StorageResult<()> {
        let (flag_type_str, variants_json) = flag_type_to_db(&flag.flag_type);
        let tags_json = serde_json::to_string(&flag.tags)?;

        let result = sqlx::query(
            r#"
            INSERT INTO flags (id, project_id, key, name, description, flag_type, variants, tags,
                               created_at, updated_at, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(flag.id.0.to_string())
        .bind(flag.project_id.0.to_string())
        .bind(flag.key.as_str())
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(flag_type_str)
        .bind(variants_json)
        .bind(tags_json)
        .bind(flag.created_at)
        .bind(flag.updated_at)
        .bind(&flag.created_by.0)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(StorageError::duplicate("Flag", "key", flag.key.as_str()))
            },
            Err(e) => Err(e.into()),
        }
    }

    async fn update(&self, flag: &Flag) -> StorageResult<()> {
        let (flag_type_str, variants_json) = flag_type_to_db(&flag.flag_type);
        let tags_json = serde_json::to_string(&flag.tags)?;

        let result = sqlx::query(
            r#"
            UPDATE flags
            SET key = $2, name = $3, description = $4, flag_type = $5, variants = $6,
                tags = $7, updated_at = $8
            WHERE id = $1
            "#,
        )
        .bind(flag.id.0.to_string())
        .bind(flag.key.as_str())
        .bind(&flag.name)
        .bind(&flag.description)
        .bind(flag_type_str)
        .bind(variants_json)
        .bind(tags_json)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found("Flag", "id", flag.id.0.to_string()));
        }

        Ok(())
    }

    async fn delete(&self, id: FlagId) -> StorageResult<()> {
        let result = sqlx::query("DELETE FROM flags WHERE id = $1")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found("Flag", "id", id.0.to_string()));
        }

        Ok(())
    }
}

fn row_to_flag(row: &sqlx::postgres::PgRow) -> StorageResult<Flag> {
    let id: String = row.try_get("id")?;
    let project_id: String = row.try_get("project_id")?;
    let key: String = row.try_get("key")?;
    let name: String = row.try_get("name")?;
    let description: Option<String> = row.try_get("description")?;
    let flag_type_str: String = row.try_get("flag_type")?;
    let variants_json: Option<String> = row.try_get("variants")?;
    let tags_json: Option<String> = row.try_get("tags")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let created_by: String = row.try_get("created_by")?;

    let flag_type = db_to_flag_type(&flag_type_str, variants_json.as_deref())?;
    let tags: Vec<String> = tags_json
        .map(|j| serde_json::from_str(&j))
        .transpose()?
        .unwrap_or_default();

    let flag_key = FlagKey::try_new(&key).ok_or_else(|| {
        StorageError::Configuration(format!("Invalid flag key in database: {}", key))
    })?;

    Ok(Flag {
        id: FlagId::from_uuid(Uuid::parse_str(&id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        project_id: ProjectId::from_uuid(Uuid::parse_str(&project_id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        key: flag_key,
        name,
        description,
        flag_type,
        environments: HashMap::new(), // Loaded separately from flag_environments table
        tags,
        created_at,
        updated_at,
        created_by: UserId::new(created_by),
    })
}

fn flag_type_to_db(flag_type: &FlagType) -> (&'static str, Option<String>) {
    match flag_type {
        FlagType::Boolean => ("boolean", None),
        FlagType::String { variants } => {
            let json = serde_json::to_string(variants).ok();
            ("string", json)
        },
    }
}

fn db_to_flag_type(type_str: &str, variants_json: Option<&str>) -> StorageResult<FlagType> {
    match type_str {
        "boolean" => Ok(FlagType::Boolean),
        "string" => {
            let variants: Vec<String> = variants_json
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default();
            Ok(FlagType::String { variants })
        },
        other => Err(StorageError::Configuration(format!(
            "Unknown flag type: {}",
            other
        ))),
    }
}
