//! PostgreSQL environment repository implementation.

use sqlx::{Pool, Postgres, Row};
use uuid::Uuid;

use flaps_core::{Environment, EnvironmentId, ProjectId};

use crate::error::{StorageError, StorageResult};
use crate::traits::EnvironmentRepository;

/// PostgreSQL implementation of the environment repository.
#[derive(Debug, Clone)]
pub struct PostgresEnvironmentRepository {
    pool: Pool<Postgres>,
}

impl PostgresEnvironmentRepository {
    /// Creates a new PostgreSQL environment repository.
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

impl EnvironmentRepository for PostgresEnvironmentRepository {
    async fn get_by_id(&self, id: EnvironmentId) -> StorageResult<Option<Environment>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, color, is_production, sort_order
            FROM environments
            WHERE id = $1
            "#,
        )
        .bind(id.0.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_environment(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_by_key(
        &self,
        project_id: ProjectId,
        key: &str,
    ) -> StorageResult<Option<Environment>> {
        let row = sqlx::query(
            r#"
            SELECT id, project_id, key, name, color, is_production, sort_order
            FROM environments
            WHERE project_id = $1 AND key = $2
            "#,
        )
        .bind(project_id.0.to_string())
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(row_to_environment(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_by_project(&self, project_id: ProjectId) -> StorageResult<Vec<Environment>> {
        let rows = sqlx::query(
            r#"
            SELECT id, project_id, key, name, color, is_production, sort_order
            FROM environments
            WHERE project_id = $1
            ORDER BY sort_order ASC, name ASC
            "#,
        )
        .bind(project_id.0.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_environment).collect()
    }

    async fn create(&self, environment: &Environment) -> StorageResult<()> {
        let result = sqlx::query(
            r#"
            INSERT INTO environments (id, project_id, key, name, color, is_production, sort_order)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(environment.id.0.to_string())
        .bind(environment.project_id.0.to_string())
        .bind(&environment.key)
        .bind(&environment.name)
        .bind(&environment.color)
        .bind(environment.is_production)
        .bind(environment.order as i32)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => Err(
                StorageError::duplicate("Environment", "key", &environment.key),
            ),
            Err(e) => Err(e.into()),
        }
    }

    async fn update(&self, environment: &Environment) -> StorageResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE environments
            SET key = $2, name = $3, color = $4, is_production = $5, sort_order = $6
            WHERE id = $1
            "#,
        )
        .bind(environment.id.0.to_string())
        .bind(&environment.key)
        .bind(&environment.name)
        .bind(&environment.color)
        .bind(environment.is_production)
        .bind(environment.order as i32)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found(
                "Environment",
                "id",
                environment.id.0.to_string(),
            ));
        }

        Ok(())
    }

    async fn delete(&self, id: EnvironmentId) -> StorageResult<()> {
        let result = sqlx::query("DELETE FROM environments WHERE id = $1")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(StorageError::not_found(
                "Environment",
                "id",
                id.0.to_string(),
            ));
        }

        Ok(())
    }
}

fn row_to_environment(row: &sqlx::postgres::PgRow) -> StorageResult<Environment> {
    let id: String = row.try_get("id")?;
    let project_id: String = row.try_get("project_id")?;
    let key: String = row.try_get("key")?;
    let name: String = row.try_get("name")?;
    let color: Option<String> = row.try_get("color")?;
    let is_production: bool = row.try_get("is_production")?;
    let sort_order: i32 = row.try_get("sort_order")?;

    Ok(Environment {
        id: EnvironmentId::from_uuid(Uuid::parse_str(&id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        project_id: ProjectId::from_uuid(Uuid::parse_str(&project_id).map_err(|e| {
            StorageError::Configuration(format!("Invalid UUID in database: {}", e))
        })?),
        key,
        name,
        color,
        is_production,
        order: sort_order as u32,
    })
}
