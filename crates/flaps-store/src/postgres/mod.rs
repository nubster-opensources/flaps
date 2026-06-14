//! PostgreSQL backend: pool construction, migrations and repository implementations.

use std::time::Duration;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use sqlx::{
    Executor, Pool, Postgres, Transaction,
    migrate::{Migration, MigrationType, Migrator},
};

use flaps_domain::{
    Environment, EnvironmentKey, ExternalRef, Flag, FlagEnvConfig, FlagKey, ManagedBy, Project,
    ProjectKey, Segment, SegmentKey,
};

use crate::{
    account::{AccountRecord, NewSession},
    audit::{AuditRecord, postgres::append_audit},
    error::{StoreError, StoreResult},
    hash::KeyHasher,
    repository::{
        account::{AccountRepository, SessionRepository},
        audit_log::AuditLogRepository,
        environment::EnvironmentRepository,
        flag::FlagRepository,
        flag_env_config::FlagEnvConfigRepository,
        project::ProjectRepository,
        sdk_key::SdkKeyRepository,
        segment::SegmentRepository,
        transaction::{TransactionalStore, WriteSession},
    },
    sdk_key::{NewSdkKey, SdkKeyRecord, SdkKeyScope},
};

// ---------------------------------------------------------------------------
// Crypto helpers (argon2id + token generation) - mirrored from sqlite backend
// ---------------------------------------------------------------------------

fn hash_password(password: &str) -> StoreResult<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| {
            StoreError::Serialization(
                serde_json::from_str::<serde_json::Value>(&format!("\"argon2 error: {e}\""))
                    .unwrap_err(),
            )
        })
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn generate_token() -> String {
    use argon2::password_hash::rand_core::RngCore;
    let mut bytes = [0u8; 32];
    argon2::password_hash::rand_core::OsRng.fill_bytes(&mut bytes);
    bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

fn secs_to_rfc3339(secs: u64) -> String {
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3_600) % 24;
    let days = secs / 86_400;
    let (year, month, day) = crate::clock::days_to_ymd_pub(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

// ---------------------------------------------------------------------------
// Type aliases for query row tuples (avoids type_complexity lint)
// ---------------------------------------------------------------------------

type ProjectRow = (String, String, Option<String>, Option<String>, String);
type EnvRow = (String, String, Option<String>, String);
type FlagRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    serde_json::Value,
);
type SegmentRow = (String, String, serde_json::Value);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn managed_by_str(m: ManagedBy) -> &'static str {
    match m {
        ManagedBy::Local => "local",
        ManagedBy::Federated => "federated",
    }
}

fn managed_by_from_str(s: &str) -> StoreResult<ManagedBy> {
    match s {
        "local" => Ok(ManagedBy::Local),
        "federated" => Ok(ManagedBy::Federated),
        other => Err(StoreError::Serialization(
            serde_json::from_str::<serde_json::Value>(&format!("\"unknown managed_by: {other}\""))
                .unwrap_err(),
        )),
    }
}

fn domain_key_err(e: &flaps_domain::DomainError) -> StoreError {
    StoreError::Serialization(
        serde_json::from_str::<serde_json::Value>(&format!("\"{e}\"")).unwrap_err(),
    )
}

fn row_to_project(
    k: String,
    name: String,
    desc: Option<String>,
    ext_ref: Option<String>,
    mb: &str,
) -> StoreResult<Project> {
    Ok(Project {
        key: ProjectKey::new(k).map_err(|e| domain_key_err(&e))?,
        name,
        description: desc,
        external_ref: ext_ref.map(ExternalRef::new),
        managed_by: managed_by_from_str(mb)?,
    })
}

fn row_to_environment(
    k: String,
    name: String,
    ext_ref: Option<String>,
    mb: &str,
) -> StoreResult<Environment> {
    Ok(Environment {
        key: EnvironmentKey::new(k).map_err(|e| domain_key_err(&e))?,
        name,
        external_ref: ext_ref.map(ExternalRef::new),
        managed_by: managed_by_from_str(mb)?,
    })
}

// ---------------------------------------------------------------------------
// Generic read helpers
// ---------------------------------------------------------------------------

async fn do_get_project<'e, E>(executor: E, key: &ProjectKey) -> StoreResult<Option<Project>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<ProjectRow> = sqlx::query_as(
        "SELECT key, name, description, external_ref, managed_by FROM projects WHERE key = $1",
    )
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, desc, ext_ref, mb)| row_to_project(k, name, desc, ext_ref, &mb))
        .transpose()
}

async fn do_get_environment<'e, E>(
    executor: E,
    project: &ProjectKey,
    key: &EnvironmentKey,
) -> StoreResult<Option<Environment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<EnvRow> = sqlx::query_as(
        "SELECT key, name, external_ref, managed_by FROM environments WHERE project_key = $1 AND key = $2",
    )
    .bind(project.as_str())
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, ext_ref, mb)| row_to_environment(k, name, ext_ref, &mb))
        .transpose()
}

async fn do_get_flag<'e, E>(
    executor: E,
    project: &ProjectKey,
    key: &FlagKey,
) -> StoreResult<Option<Flag>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<FlagRow> =
        sqlx::query_as(
            "SELECT key, name, description, flag_type, value_type, variants_json FROM flags WHERE project_key = $1 AND key = $2",
        )
        .bind(project.as_str())
        .bind(key.as_str())
        .fetch_optional(executor)
        .await?;

    row.map(|(k, name, desc, ft, vt, vj)| {
        Ok(Flag {
            key: FlagKey::new(k).map_err(|e| domain_key_err(&e))?,
            name,
            description: desc,
            flag_type: serde_json::from_str(&format!(r#""{ft}""#))?,
            value_type: serde_json::from_str(&format!(r#""{vt}""#))?,
            variants: serde_json::from_value(vj)?,
        })
    })
    .transpose()
}

async fn do_get_segment<'e, E>(
    executor: E,
    project: &ProjectKey,
    key: &SegmentKey,
) -> StoreResult<Option<Segment>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<SegmentRow> = sqlx::query_as(
        "SELECT key, name, match_json FROM segments WHERE project_key = $1 AND key = $2",
    )
    .bind(project.as_str())
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, mj)| {
        Ok(Segment {
            key: SegmentKey::new(k).map_err(|e| domain_key_err(&e))?,
            name,
            match_expr: serde_json::from_value(mj)?,
        })
    })
    .transpose()
}

async fn do_get_flag_env_config<'e, E>(
    executor: E,
    project: &ProjectKey,
    flag: &FlagKey,
    environment: &EnvironmentKey,
) -> StoreResult<Option<FlagEnvConfig>>
where
    E: Executor<'e, Database = Postgres>,
{
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT config_json FROM flag_env_configs WHERE project_key = $1 AND flag_key = $2 AND environment_key = $3",
    )
    .bind(project.as_str())
    .bind(flag.as_str())
    .bind(environment.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(cj,)| Ok(serde_json::from_value(cj)?)).transpose()
}

// ---------------------------------------------------------------------------
// Generic write helpers
// ---------------------------------------------------------------------------

async fn do_upsert_project<'e, E>(executor: E, project: &Project) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let external_ref = project.external_ref.as_ref().map(|r| r.as_str().to_owned());
    let managed_by = managed_by_str(project.managed_by);
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO projects (key, name, description, external_ref, managed_by, created_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7)
          ON CONFLICT(key) DO UPDATE SET
              name         = EXCLUDED.name,
              description  = EXCLUDED.description,
              external_ref = EXCLUDED.external_ref,
              managed_by   = EXCLUDED.managed_by,
              updated_at   = EXCLUDED.updated_at",
    )
    .bind(project.key.as_str())
    .bind(&project.name)
    .bind(project.description.as_deref())
    .bind(external_ref)
    .bind(managed_by)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(StoreError::Conflict(format!(
                "external_ref already used: {}",
                project.external_ref.as_ref().map_or("", |r| r.as_str())
            )))
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

async fn do_upsert_environment<'e, E>(
    executor: E,
    project: &ProjectKey,
    env: &Environment,
) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let external_ref = env.external_ref.as_ref().map(|r| r.as_str().to_owned());
    let managed_by = managed_by_str(env.managed_by);
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO environments (project_key, key, name, external_ref, managed_by, created_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name         = EXCLUDED.name,
              external_ref = EXCLUDED.external_ref,
              managed_by   = EXCLUDED.managed_by,
              updated_at   = EXCLUDED.updated_at",
    )
    .bind(project.as_str())
    .bind(env.key.as_str())
    .bind(&env.name)
    .bind(external_ref)
    .bind(managed_by)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            Err(StoreError::Conflict(format!(
                "external_ref already used: {}",
                env.external_ref.as_ref().map_or("", |r| r.as_str())
            )))
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

async fn do_upsert_flag<'e, E>(executor: E, project: &ProjectKey, flag: &Flag) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let variants_json: serde_json::Value = serde_json::to_value(&flag.variants)?;
    let flag_type = serde_json::to_string(&flag.flag_type)?;
    let value_type = serde_json::to_string(&flag.value_type)?;
    let now = crate::clock::now_rfc3339();

    sqlx::query(
        r"INSERT INTO flags (project_key, key, name, description, flag_type, value_type, variants_json, created_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name          = EXCLUDED.name,
              description   = EXCLUDED.description,
              flag_type     = EXCLUDED.flag_type,
              value_type    = EXCLUDED.value_type,
              variants_json = EXCLUDED.variants_json,
              updated_at    = EXCLUDED.updated_at",
    )
    .bind(project.as_str())
    .bind(flag.key.as_str())
    .bind(&flag.name)
    .bind(flag.description.as_deref())
    .bind(flag_type.trim_matches('"'))
    .bind(value_type.trim_matches('"'))
    .bind(variants_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await?;

    Ok(())
}

async fn do_upsert_segment<'e, E>(
    executor: E,
    project: &ProjectKey,
    segment: &Segment,
) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let match_json: serde_json::Value = serde_json::to_value(&segment.match_expr)?;
    let now = crate::clock::now_rfc3339();

    sqlx::query(
        r"INSERT INTO segments (project_key, key, name, match_json, created_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name       = EXCLUDED.name,
              match_json = EXCLUDED.match_json,
              updated_at = EXCLUDED.updated_at",
    )
    .bind(project.as_str())
    .bind(segment.key.as_str())
    .bind(&segment.name)
    .bind(match_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await?;

    Ok(())
}

async fn do_upsert_flag_env_config<'e, E>(
    executor: E,
    project: &ProjectKey,
    flag: &FlagKey,
    environment: &EnvironmentKey,
    config: &FlagEnvConfig,
) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let config_json: serde_json::Value = serde_json::to_value(config)?;
    let now = crate::clock::now_rfc3339();

    sqlx::query(
        r"INSERT INTO flag_env_configs (project_key, flag_key, environment_key, config_json, created_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6)
          ON CONFLICT(project_key, flag_key, environment_key) DO UPDATE SET
              config_json = EXCLUDED.config_json,
              updated_at  = EXCLUDED.updated_at",
    )
    .bind(project.as_str())
    .bind(flag.as_str())
    .bind(environment.as_str())
    .bind(config_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Embedded migrations
// ---------------------------------------------------------------------------

/// Returns a [`Migrator`] with the PostgreSQL schema embedded at compile time.
///
/// Avoids the `sqlx/macros` feature (which pulls in `sqlx-mysql` and transitively
/// the vulnerable `rsa` crate) while keeping migrations in the binary.
fn embedded_migrator() -> Migrator {
    use std::borrow::Cow;

    static MIGRATIONS: std::sync::OnceLock<Vec<Migration>> = std::sync::OnceLock::new();
    let migrations = MIGRATIONS.get_or_init(|| {
        vec![
            Migration::new(
                1,
                Cow::Borrowed("init"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!("../../migrations/postgres/0001_init.sql")),
                false,
            ),
            Migration::new(
                2,
                Cow::Borrowed("audit_log"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!("../../migrations/postgres/0002_audit_log.sql")),
                false,
            ),
            Migration::new(
                3,
                Cow::Borrowed("accounts"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!("../../migrations/postgres/0003_accounts.sql")),
                false,
            ),
            Migration::new(
                4,
                Cow::Borrowed("sdk_key_revocation"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!(
                    "../../migrations/postgres/0004_sdk_key_revocation.sql"
                )),
                false,
            ),
        ]
    });

    Migrator {
        migrations: Cow::Owned(migrations.clone()),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    }
}

// ---------------------------------------------------------------------------
// PostgresStore
// ---------------------------------------------------------------------------

/// PostgreSQL-backed store: connection pool + HMAC hasher.
#[derive(Clone)]
pub struct PostgresStore {
    pool: Pool<Postgres>,
    hasher: KeyHasher,
}

impl PostgresStore {
    /// Connects to the given Postgres URL and runs embedded migrations.
    ///
    /// # Errors
    /// Returns [`StoreError`] if the connection or migrations fail.
    pub async fn connect(url: &str, hasher: KeyHasher) -> StoreResult<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new().connect(url).await?;
        embedded_migrator().run(&pool).await?;
        Ok(Self { pool, hasher })
    }
}

// ---------------------------------------------------------------------------
// ProjectRepository for PostgresStore
// ---------------------------------------------------------------------------

impl ProjectRepository for PostgresStore {
    async fn upsert_project(&self, actor: &str, project: &Project) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_project(&mut *tx, &project.key).await?;
        do_upsert_project(&mut *tx, project).await?;
        let action = if before.is_some() {
            "project.updated"
        } else {
            "project.created"
        };
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: "project".to_owned(),
            entity_id: project.key.as_str().to_owned(),
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(project).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_project(&self, key: &ProjectKey) -> StoreResult<Option<Project>> {
        do_get_project(&self.pool, key).await
    }

    async fn list_projects(&self) -> StoreResult<Vec<Project>> {
        let rows: Vec<ProjectRow> =
            sqlx::query_as("SELECT key, name, description, external_ref, managed_by FROM projects")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|(k, name, desc, ext_ref, mb)| row_to_project(k, name, desc, ext_ref, &mb))
            .collect()
    }

    async fn delete_project(&self, actor: &str, key: &ProjectKey) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_project(&mut *tx, key).await?;
        let Some(before_val) = before else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("DELETE FROM projects WHERE key = $1")
            .bind(key.as_str())
            .execute(&mut *tx)
            .await?;
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "project.deleted".to_owned(),
            entity_type: "project".to_owned(),
            entity_id: key.as_str().to_owned(),
            before: Some(serde_json::to_value(&before_val).map_err(StoreError::Serialization)?),
            after: None,
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EnvironmentRepository for PostgresStore
// ---------------------------------------------------------------------------

impl EnvironmentRepository for PostgresStore {
    async fn upsert_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        env: &Environment,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_environment(&mut *tx, project, &env.key).await?;
        do_upsert_environment(&mut *tx, project, env).await?;
        let action = if before.is_some() {
            "environment.updated"
        } else {
            "environment.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), env.key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: "environment".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(env).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_environment(
        &self,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<Option<Environment>> {
        do_get_environment(&self.pool, project, key).await
    }

    async fn list_environments(&self, project: &ProjectKey) -> StoreResult<Vec<Environment>> {
        let rows: Vec<EnvRow> = sqlx::query_as(
            "SELECT key, name, external_ref, managed_by FROM environments WHERE project_key = $1",
        )
        .bind(project.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(k, name, ext_ref, mb)| row_to_environment(k, name, ext_ref, &mb))
            .collect()
    }

    async fn delete_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_environment(&mut *tx, project, key).await?;
        let Some(before_val) = before else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("DELETE FROM environments WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&mut *tx)
            .await?;
        let entity_id = format!("{}/{}", project.as_str(), key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "environment.deleted".to_owned(),
            entity_type: "environment".to_owned(),
            entity_id,
            before: Some(serde_json::to_value(&before_val).map_err(StoreError::Serialization)?),
            after: None,
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FlagRepository for PostgresStore
// ---------------------------------------------------------------------------

impl FlagRepository for PostgresStore {
    async fn upsert_flag(&self, actor: &str, project: &ProjectKey, flag: &Flag) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_flag(&mut *tx, project, &flag.key).await?;
        do_upsert_flag(&mut *tx, project, flag).await?;
        let action = if before.is_some() {
            "flag.updated"
        } else {
            "flag.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), flag.key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: "flag".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(flag).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_flag(&self, project: &ProjectKey, key: &FlagKey) -> StoreResult<Option<Flag>> {
        do_get_flag(&self.pool, project, key).await
    }

    async fn list_flags(&self, project: &ProjectKey) -> StoreResult<Vec<Flag>> {
        let rows: Vec<FlagRow> =
            sqlx::query_as(
                "SELECT key, name, description, flag_type, value_type, variants_json FROM flags WHERE project_key = $1",
            )
            .bind(project.as_str())
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter()
            .map(|(k, name, desc, ft, vt, vj)| {
                Ok(Flag {
                    key: FlagKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    description: desc,
                    flag_type: serde_json::from_str(&format!(r#""{ft}""#))?,
                    value_type: serde_json::from_str(&format!(r#""{vt}""#))?,
                    variants: serde_json::from_value(vj)?,
                })
            })
            .collect()
    }

    async fn delete_flag(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &FlagKey,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_flag(&mut *tx, project, key).await?;
        let Some(before_val) = before else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("DELETE FROM flags WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&mut *tx)
            .await?;
        let entity_id = format!("{}/{}", project.as_str(), key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "flag.deleted".to_owned(),
            entity_type: "flag".to_owned(),
            entity_id,
            before: Some(serde_json::to_value(&before_val).map_err(StoreError::Serialization)?),
            after: None,
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SegmentRepository for PostgresStore
// ---------------------------------------------------------------------------

impl SegmentRepository for PostgresStore {
    async fn upsert_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        segment: &Segment,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_segment(&mut *tx, project, &segment.key).await?;
        do_upsert_segment(&mut *tx, project, segment).await?;
        let action = if before.is_some() {
            "segment.updated"
        } else {
            "segment.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), segment.key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: "segment".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(segment).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_segment(
        &self,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> StoreResult<Option<Segment>> {
        do_get_segment(&self.pool, project, key).await
    }

    async fn list_segments(&self, project: &ProjectKey) -> StoreResult<Vec<Segment>> {
        let rows: Vec<SegmentRow> =
            sqlx::query_as("SELECT key, name, match_json FROM segments WHERE project_key = $1")
                .bind(project.as_str())
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|(k, name, mj)| {
                Ok(Segment {
                    key: SegmentKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    match_expr: serde_json::from_value(mj)?,
                })
            })
            .collect()
    }

    async fn delete_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_segment(&mut *tx, project, key).await?;
        let Some(before_val) = before else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query("DELETE FROM segments WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&mut *tx)
            .await?;
        let entity_id = format!("{}/{}", project.as_str(), key.as_str());
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "segment.deleted".to_owned(),
            entity_type: "segment".to_owned(),
            entity_id,
            before: Some(serde_json::to_value(&before_val).map_err(StoreError::Serialization)?),
            after: None,
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FlagEnvConfigRepository for PostgresStore
// ---------------------------------------------------------------------------

impl FlagEnvConfigRepository for PostgresStore {
    async fn upsert_flag_env_config(
        &self,
        actor: &str,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_flag_env_config(&mut *tx, project, flag, environment).await?;
        do_upsert_flag_env_config(&mut *tx, project, flag, environment, config).await?;
        let action = if before.is_some() {
            "flag_env_config.updated"
        } else {
            "flag_env_config.created"
        };
        let entity_id = format!(
            "{}/{}/{}",
            project.as_str(),
            flag.as_str(),
            environment.as_str()
        );
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: "flag_env_config".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(config).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn get_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<Option<FlagEnvConfig>> {
        do_get_flag_env_config(&self.pool, project, flag, environment).await
    }

    async fn delete_flag_env_config(
        &self,
        actor: &str,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;
        let before = do_get_flag_env_config(&mut *tx, project, flag, environment).await?;
        let Some(before_val) = before else {
            tx.commit().await?;
            return Ok(());
        };
        sqlx::query(
            "DELETE FROM flag_env_configs WHERE project_key = $1 AND flag_key = $2 AND environment_key = $3",
        )
        .bind(project.as_str())
        .bind(flag.as_str())
        .bind(environment.as_str())
        .execute(&mut *tx)
        .await?;
        let entity_id = format!(
            "{}/{}/{}",
            project.as_str(),
            flag.as_str(),
            environment.as_str()
        );
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "flag_env_config.deleted".to_owned(),
            entity_type: "flag_env_config".to_owned(),
            entity_id,
            before: Some(serde_json::to_value(&before_val).map_err(StoreError::Serialization)?),
            after: None,
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *tx, &record).await?;
        tx.commit().await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SdkKeyRepository for PostgresStore
// ---------------------------------------------------------------------------

impl SdkKeyRepository for PostgresStore {
    async fn create_sdk_key(
        &self,
        raw_key: &str,
        new_key: &NewSdkKey,
    ) -> StoreResult<SdkKeyRecord> {
        let key_hash = self.hasher.hash(raw_key);
        let prefix = raw_key.chars().take(12).collect::<String>();
        let kind_str = serde_json::to_string(&new_key.kind)?;
        let kind_str = kind_str.trim_matches('"');
        let now = crate::clock::now_rfc3339();

        sqlx::query(
            r"INSERT INTO sdk_keys (key_hash, prefix, kind, project_key, environment_key, created_at)
              VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&key_hash)
        .bind(&prefix)
        .bind(kind_str)
        .bind(new_key.scope.project_key.as_str())
        .bind(new_key.scope.environment_key.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(SdkKeyRecord {
            prefix,
            kind: new_key.kind,
            scope: new_key.scope.clone(),
            created_at: now,
        })
    }

    async fn find_sdk_key(&self, raw_key: &str) -> StoreResult<Option<SdkKeyRecord>> {
        let key_hash = self.hasher.hash(raw_key);

        let row: Option<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT prefix, kind, project_key, environment_key, created_at \
             FROM sdk_keys WHERE key_hash = $1 AND revoked_at IS NULL",
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(prefix, kind_str, proj_key, env_key, created_at)| {
            let kind = serde_json::from_str(&format!(r#""{kind_str}""#))?;
            Ok(SdkKeyRecord {
                prefix,
                kind,
                scope: SdkKeyScope {
                    project_key: ProjectKey::new(proj_key).map_err(|e| domain_key_err(&e))?,
                    environment_key: EnvironmentKey::new(env_key)
                        .map_err(|e| domain_key_err(&e))?,
                },
                created_at,
            })
        })
        .transpose()
    }

    async fn list_sdk_keys(
        &self,
        _actor: &str,
        scope: &SdkKeyScope,
    ) -> StoreResult<Vec<SdkKeyRecord>> {
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT prefix, kind, project_key, environment_key, created_at \
             FROM sdk_keys \
             WHERE project_key = $1 AND environment_key = $2 \
             ORDER BY created_at ASC",
        )
        .bind(scope.project_key.as_str())
        .bind(scope.environment_key.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(prefix, kind_str, proj_key, env_key, created_at)| {
                let kind = serde_json::from_str(&format!(r#""{kind_str}""#))?;
                Ok(SdkKeyRecord {
                    prefix,
                    kind,
                    scope: SdkKeyScope {
                        project_key: ProjectKey::new(proj_key).map_err(|e| domain_key_err(&e))?,
                        environment_key: EnvironmentKey::new(env_key)
                            .map_err(|e| domain_key_err(&e))?,
                    },
                    created_at,
                })
            })
            .collect()
    }

    async fn revoke_sdk_key(
        &self,
        actor: &str,
        project: &ProjectKey,
        environment: &EnvironmentKey,
        prefix: &str,
    ) -> StoreResult<()> {
        let now = crate::clock::now_rfc3339();
        sqlx::query(
            "UPDATE sdk_keys SET revoked_at = $1 \
             WHERE project_key = $2 AND environment_key = $3 AND prefix = $4 AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(project.as_str())
        .bind(environment.as_str())
        .bind(prefix)
        .execute(&self.pool)
        .await?;

        let entity_id = format!("{}/{}/{}", project.as_str(), environment.as_str(), prefix);
        let record = AuditRecord {
            actor: actor.to_owned(),
            action: "sdk_key.revoked".to_owned(),
            entity_type: "sdk_key".to_owned(),
            entity_id,
            before: None,
            after: None,
            occurred_at: now,
        };
        append_audit(&self.pool, &record).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AccountRepository for PostgresStore
// ---------------------------------------------------------------------------

impl AccountRepository for PostgresStore {
    async fn create_account(
        &self,
        actor: &str,
        username: &str,
        password: &str,
    ) -> StoreResult<AccountRecord> {
        let id = uuid::Uuid::new_v4().to_string();
        let password_hash = hash_password(password)?;
        let now = crate::clock::now_rfc3339();

        let result = sqlx::query(
            r"INSERT INTO accounts (id, username, password_hash, is_active, created_at)
              VALUES ($1, $2, $3, true, $4)",
        )
        .bind(&id)
        .bind(username)
        .bind(&password_hash)
        .bind(&now)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(StoreError::Conflict(format!(
                    "username already taken: {username}"
                )));
            }
            Err(e) => return Err(StoreError::Sqlx(e)),
        }

        let record = AccountRecord {
            id: id.clone(),
            username: username.to_owned(),
        };

        let audit = AuditRecord {
            actor: actor.to_owned(),
            action: "account.created".to_owned(),
            entity_type: "account".to_owned(),
            entity_id: id,
            before: None,
            after: None,
            occurred_at: now,
        };
        append_audit(&self.pool, &audit).await?;

        Ok(record)
    }

    async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> StoreResult<Option<AccountRecord>> {
        let row: Option<(String, String, String, bool)> = sqlx::query_as(
            "SELECT id, username, password_hash, is_active FROM accounts WHERE username = $1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        let Some((id, uname, hash, is_active)) = row else {
            return Ok(None);
        };

        if !is_active {
            return Ok(None);
        }

        if verify_password(password, &hash) {
            Ok(Some(AccountRecord {
                id,
                username: uname,
            }))
        } else {
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// SessionRepository for PostgresStore
// ---------------------------------------------------------------------------

impl SessionRepository for PostgresStore {
    async fn create_session(&self, account_id: &str, ttl: Duration) -> StoreResult<NewSession> {
        let raw_token = generate_token();
        let token_hash = self.hasher.hash(&raw_token);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires_secs = now_secs.saturating_add(ttl.as_secs());
        let expires_at = secs_to_rfc3339(expires_secs);

        sqlx::query(
            r"INSERT INTO sessions (token_hash, account_id, expires_at)
              VALUES ($1, $2, $3)",
        )
        .bind(&token_hash)
        .bind(account_id)
        .bind(&expires_at)
        .execute(&self.pool)
        .await?;

        Ok(NewSession {
            token: raw_token,
            expires_at,
        })
    }

    async fn resolve_session(&self, raw_token: &str) -> StoreResult<Option<AccountRecord>> {
        let token_hash = self.hasher.hash(raw_token);
        let now = crate::clock::now_rfc3339();

        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT a.id, a.username \
             FROM sessions s \
             JOIN accounts a ON a.id = s.account_id \
             WHERE s.token_hash = $1 \
               AND s.revoked_at IS NULL \
               AND s.expires_at > $2 \
               AND a.is_active = true",
        )
        .bind(&token_hash)
        .bind(&now)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(id, username)| AccountRecord { id, username }))
    }

    async fn revoke_session(&self, raw_token: &str) -> StoreResult<()> {
        let token_hash = self.hasher.hash(raw_token);
        let now = crate::clock::now_rfc3339();

        sqlx::query(
            "UPDATE sessions SET revoked_at = $1 WHERE token_hash = $2 AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(&token_hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AuditLogRepository for PostgresStore
// ---------------------------------------------------------------------------

type AuditRow = (
    String,
    String,
    String,
    String,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    String,
);

fn row_to_audit_record(
    actor: String,
    action: String,
    entity_type: String,
    entity_id: String,
    before_json: Option<serde_json::Value>,
    after_json: Option<serde_json::Value>,
    occurred_at: String,
) -> AuditRecord {
    AuditRecord {
        actor,
        action,
        entity_type,
        entity_id,
        before: before_json,
        after: after_json,
        occurred_at,
    }
}

impl AuditLogRepository for PostgresStore {
    async fn list_audit_entries(&self) -> StoreResult<Vec<AuditRecord>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT actor, action, entity_type, entity_id, before_json, after_json, occurred_at \
             FROM audit_log ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(actor, action, entity_type, entity_id, before_json, after_json, occurred_at)| {
                    row_to_audit_record(
                        actor,
                        action,
                        entity_type,
                        entity_id,
                        before_json,
                        after_json,
                        occurred_at,
                    )
                },
            )
            .collect())
    }

    async fn audit_entries_for(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> StoreResult<Vec<AuditRecord>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT actor, action, entity_type, entity_id, before_json, after_json, occurred_at \
             FROM audit_log WHERE entity_type = $1 AND entity_id = $2 ORDER BY id ASC",
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(actor, action, entity_type, entity_id, before_json, after_json, occurred_at)| {
                    row_to_audit_record(
                        actor,
                        action,
                        entity_type,
                        entity_id,
                        before_json,
                        after_json,
                        occurred_at,
                    )
                },
            )
            .collect())
    }
}

// ---------------------------------------------------------------------------
// TransactionalStore for PostgresStore
// ---------------------------------------------------------------------------

/// A write session bound to a single PostgreSQL transaction.
pub struct PostgresWriteSession<'a> {
    tx: Transaction<'a, Postgres>,
    actor: String,
}

impl TransactionalStore for PostgresStore {
    type Session<'a> = PostgresWriteSession<'a>;

    async fn begin(&self, actor: &str) -> StoreResult<Self::Session<'_>> {
        let tx = self.pool.begin().await?;
        Ok(PostgresWriteSession {
            tx,
            actor: actor.to_owned(),
        })
    }
}

impl WriteSession for PostgresWriteSession<'_> {
    async fn upsert_project(&mut self, project: &Project) -> StoreResult<()> {
        let before = do_get_project(&mut *self.tx, &project.key).await?;
        do_upsert_project(&mut *self.tx, project).await?;
        let action = if before.is_some() {
            "project.updated"
        } else {
            "project.created"
        };
        let record = AuditRecord {
            actor: self.actor.clone(),
            action: action.to_owned(),
            entity_type: "project".to_owned(),
            entity_id: project.key.as_str().to_owned(),
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(project).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *self.tx, &record).await
    }

    async fn upsert_environment(
        &mut self,
        project: &ProjectKey,
        env: &Environment,
    ) -> StoreResult<()> {
        let before = do_get_environment(&mut *self.tx, project, &env.key).await?;
        do_upsert_environment(&mut *self.tx, project, env).await?;
        let action = if before.is_some() {
            "environment.updated"
        } else {
            "environment.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), env.key.as_str());
        let record = AuditRecord {
            actor: self.actor.clone(),
            action: action.to_owned(),
            entity_type: "environment".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(env).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *self.tx, &record).await
    }

    async fn upsert_flag(&mut self, project: &ProjectKey, flag: &Flag) -> StoreResult<()> {
        let before = do_get_flag(&mut *self.tx, project, &flag.key).await?;
        do_upsert_flag(&mut *self.tx, project, flag).await?;
        let action = if before.is_some() {
            "flag.updated"
        } else {
            "flag.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), flag.key.as_str());
        let record = AuditRecord {
            actor: self.actor.clone(),
            action: action.to_owned(),
            entity_type: "flag".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(flag).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *self.tx, &record).await
    }

    async fn upsert_segment(&mut self, project: &ProjectKey, segment: &Segment) -> StoreResult<()> {
        let before = do_get_segment(&mut *self.tx, project, &segment.key).await?;
        do_upsert_segment(&mut *self.tx, project, segment).await?;
        let action = if before.is_some() {
            "segment.updated"
        } else {
            "segment.created"
        };
        let entity_id = format!("{}/{}", project.as_str(), segment.key.as_str());
        let record = AuditRecord {
            actor: self.actor.clone(),
            action: action.to_owned(),
            entity_type: "segment".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(segment).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *self.tx, &record).await
    }

    async fn upsert_flag_env_config(
        &mut self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()> {
        let before = do_get_flag_env_config(&mut *self.tx, project, flag, environment).await?;
        do_upsert_flag_env_config(&mut *self.tx, project, flag, environment, config).await?;
        let action = if before.is_some() {
            "flag_env_config.updated"
        } else {
            "flag_env_config.created"
        };
        let entity_id = format!(
            "{}/{}/{}",
            project.as_str(),
            flag.as_str(),
            environment.as_str()
        );
        let record = AuditRecord {
            actor: self.actor.clone(),
            action: action.to_owned(),
            entity_type: "flag_env_config".to_owned(),
            entity_id,
            before: before
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(StoreError::Serialization)?,
            after: Some(serde_json::to_value(config).map_err(StoreError::Serialization)?),
            occurred_at: crate::clock::now_rfc3339(),
        };
        append_audit(&mut *self.tx, &record).await
    }

    async fn commit(self) -> StoreResult<()> {
        self.tx.commit().await?;
        Ok(())
    }
}
