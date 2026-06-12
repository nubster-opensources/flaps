//! PostgreSQL backend: pool construction, migrations and repository implementations.

use sqlx::{Executor, Pool, Postgres, Transaction};

use flaps_domain::{
    Environment, EnvironmentKey, ExternalRef, Flag, FlagEnvConfig, FlagKey, ManagedBy, Project,
    ProjectKey, Segment, SegmentKey,
};

use crate::{
    error::{StoreError, StoreResult},
    hash::KeyHasher,
    repository::{
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

// ---------------------------------------------------------------------------
// Generic write helpers
// ---------------------------------------------------------------------------

async fn do_upsert_project<'e, E>(executor: E, project: &Project) -> StoreResult<()>
where
    E: Executor<'e, Database = Postgres>,
{
    let external_ref = project.external_ref.as_ref().map(|r| r.as_str().to_owned());
    let managed_by = managed_by_str(project.managed_by);
    let now = chrono_now();

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
    let now = chrono_now();

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
    let now = chrono_now();

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
    let now = chrono_now();

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
    let now = chrono_now();

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

/// Returns a timestamp string suitable for Postgres TIMESTAMPTZ.
/// We use ISO-8601 format which Postgres parses natively.
fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}+00:00")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let y = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let d = day_of_year - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
        sqlx::migrate!("./migrations/postgres").run(&pool).await?;
        Ok(Self { pool, hasher })
    }
}

// ---------------------------------------------------------------------------
// ProjectRepository for PostgresStore
// ---------------------------------------------------------------------------

impl ProjectRepository for PostgresStore {
    async fn upsert_project(&self, project: &Project) -> StoreResult<()> {
        do_upsert_project(&self.pool, project).await
    }

    async fn get_project(&self, key: &ProjectKey) -> StoreResult<Option<Project>> {
        let row: Option<ProjectRow> = sqlx::query_as(
            "SELECT key, name, description, external_ref, managed_by FROM projects WHERE key = $1",
        )
        .bind(key.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(k, name, desc, ext_ref, mb)| {
            Ok(Project {
                key: ProjectKey::new(k).map_err(|e| domain_key_err(&e))?,
                name,
                description: desc,
                external_ref: ext_ref.map(ExternalRef::new),
                managed_by: managed_by_from_str(&mb)?,
            })
        })
        .transpose()
    }

    async fn list_projects(&self) -> StoreResult<Vec<Project>> {
        let rows: Vec<ProjectRow> =
            sqlx::query_as("SELECT key, name, description, external_ref, managed_by FROM projects")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|(k, name, desc, ext_ref, mb)| {
                Ok(Project {
                    key: ProjectKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    description: desc,
                    external_ref: ext_ref.map(ExternalRef::new),
                    managed_by: managed_by_from_str(&mb)?,
                })
            })
            .collect()
    }

    async fn delete_project(&self, key: &ProjectKey) -> StoreResult<()> {
        sqlx::query("DELETE FROM projects WHERE key = $1")
            .bind(key.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EnvironmentRepository for PostgresStore
// ---------------------------------------------------------------------------

impl EnvironmentRepository for PostgresStore {
    async fn upsert_environment(&self, project: &ProjectKey, env: &Environment) -> StoreResult<()> {
        do_upsert_environment(&self.pool, project, env).await
    }

    async fn get_environment(
        &self,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<Option<Environment>> {
        let row: Option<EnvRow> = sqlx::query_as(
            "SELECT key, name, external_ref, managed_by FROM environments WHERE project_key = $1 AND key = $2",
        )
        .bind(project.as_str())
        .bind(key.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(k, name, ext_ref, mb)| {
            Ok(Environment {
                key: EnvironmentKey::new(k).map_err(|e| domain_key_err(&e))?,
                name,
                external_ref: ext_ref.map(ExternalRef::new),
                managed_by: managed_by_from_str(&mb)?,
            })
        })
        .transpose()
    }

    async fn list_environments(&self, project: &ProjectKey) -> StoreResult<Vec<Environment>> {
        let rows: Vec<EnvRow> = sqlx::query_as(
            "SELECT key, name, external_ref, managed_by FROM environments WHERE project_key = $1",
        )
        .bind(project.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(k, name, ext_ref, mb)| {
                Ok(Environment {
                    key: EnvironmentKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    external_ref: ext_ref.map(ExternalRef::new),
                    managed_by: managed_by_from_str(&mb)?,
                })
            })
            .collect()
    }

    async fn delete_environment(
        &self,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<()> {
        sqlx::query("DELETE FROM environments WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FlagRepository for PostgresStore
// ---------------------------------------------------------------------------

impl FlagRepository for PostgresStore {
    async fn upsert_flag(&self, project: &ProjectKey, flag: &Flag) -> StoreResult<()> {
        do_upsert_flag(&self.pool, project, flag).await
    }

    async fn get_flag(&self, project: &ProjectKey, key: &FlagKey) -> StoreResult<Option<Flag>> {
        let row: Option<FlagRow> =
            sqlx::query_as(
                "SELECT key, name, description, flag_type, value_type, variants_json FROM flags WHERE project_key = $1 AND key = $2",
            )
            .bind(project.as_str())
            .bind(key.as_str())
            .fetch_optional(&self.pool)
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

    async fn delete_flag(&self, project: &ProjectKey, key: &FlagKey) -> StoreResult<()> {
        sqlx::query("DELETE FROM flags WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SegmentRepository for PostgresStore
// ---------------------------------------------------------------------------

impl SegmentRepository for PostgresStore {
    async fn upsert_segment(&self, project: &ProjectKey, segment: &Segment) -> StoreResult<()> {
        do_upsert_segment(&self.pool, project, segment).await
    }

    async fn get_segment(
        &self,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> StoreResult<Option<Segment>> {
        let row: Option<SegmentRow> = sqlx::query_as(
            "SELECT key, name, match_json FROM segments WHERE project_key = $1 AND key = $2",
        )
        .bind(project.as_str())
        .bind(key.as_str())
        .fetch_optional(&self.pool)
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

    async fn delete_segment(&self, project: &ProjectKey, key: &SegmentKey) -> StoreResult<()> {
        sqlx::query("DELETE FROM segments WHERE project_key = $1 AND key = $2")
            .bind(project.as_str())
            .bind(key.as_str())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FlagEnvConfigRepository for PostgresStore
// ---------------------------------------------------------------------------

impl FlagEnvConfigRepository for PostgresStore {
    async fn upsert_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()> {
        do_upsert_flag_env_config(&self.pool, project, flag, environment, config).await
    }

    async fn get_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<Option<FlagEnvConfig>> {
        let row: Option<(serde_json::Value,)> = sqlx::query_as(
            "SELECT config_json FROM flag_env_configs WHERE project_key = $1 AND flag_key = $2 AND environment_key = $3",
        )
        .bind(project.as_str())
        .bind(flag.as_str())
        .bind(environment.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(cj,)| Ok(serde_json::from_value(cj)?)).transpose()
    }

    async fn delete_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<()> {
        sqlx::query(
            "DELETE FROM flag_env_configs WHERE project_key = $1 AND flag_key = $2 AND environment_key = $3",
        )
        .bind(project.as_str())
        .bind(flag.as_str())
        .bind(environment.as_str())
        .execute(&self.pool)
        .await?;
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
        let now = chrono_now();

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

        let row: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT prefix, kind, project_key, environment_key FROM sdk_keys WHERE key_hash = $1",
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(prefix, kind_str, proj_key, env_key)| {
            let kind = serde_json::from_str(&format!(r#""{kind_str}""#))?;
            Ok(SdkKeyRecord {
                prefix,
                kind,
                scope: SdkKeyScope {
                    project_key: ProjectKey::new(proj_key).map_err(|e| domain_key_err(&e))?,
                    environment_key: EnvironmentKey::new(env_key)
                        .map_err(|e| domain_key_err(&e))?,
                },
                created_at: String::new(),
            })
        })
        .transpose()
    }
}

// ---------------------------------------------------------------------------
// TransactionalStore for PostgresStore
// ---------------------------------------------------------------------------

/// A write session bound to a single PostgreSQL transaction.
pub struct PostgresWriteSession<'a> {
    tx: Transaction<'a, Postgres>,
}

impl TransactionalStore for PostgresStore {
    type Session<'a> = PostgresWriteSession<'a>;

    async fn begin(&self) -> StoreResult<Self::Session<'_>> {
        let tx = self.pool.begin().await?;
        Ok(PostgresWriteSession { tx })
    }
}

impl WriteSession for PostgresWriteSession<'_> {
    async fn upsert_project(&mut self, project: &Project) -> StoreResult<()> {
        do_upsert_project(&mut *self.tx, project).await
    }

    async fn upsert_environment(
        &mut self,
        project: &ProjectKey,
        env: &Environment,
    ) -> StoreResult<()> {
        do_upsert_environment(&mut *self.tx, project, env).await
    }

    async fn upsert_flag(&mut self, project: &ProjectKey, flag: &Flag) -> StoreResult<()> {
        do_upsert_flag(&mut *self.tx, project, flag).await
    }

    async fn upsert_segment(&mut self, project: &ProjectKey, segment: &Segment) -> StoreResult<()> {
        do_upsert_segment(&mut *self.tx, project, segment).await
    }

    async fn upsert_flag_env_config(
        &mut self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()> {
        do_upsert_flag_env_config(&mut *self.tx, project, flag, environment, config).await
    }

    async fn commit(self) -> StoreResult<()> {
        self.tx.commit().await?;
        Ok(())
    }
}
