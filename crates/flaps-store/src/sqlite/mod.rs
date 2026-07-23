//! SQLite backend: pool construction, migrations and repository implementations.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use sqlx::{
    Executor, Pool, Sqlite, Transaction,
    migrate::{Migration, MigrationType, Migrator},
};

use flaps_domain::{
    Environment, EnvironmentKey, ExternalRef, Flag, FlagEnvConfig, FlagKey, ManagedBy, Project,
    ProjectKey, Segment, SegmentKey,
};

use crate::{
    account::{AccountRecord, NewSession},
    audit::{AuditRecord, sqlite::append_audit},
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
// Crypto helpers (argon2id + token generation)
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

/// Verifies a password on a blocking thread.
///
/// Argon2 is CPU-bound by design. Called directly inside an `async fn` it runs
/// on a runtime worker thread, so a burst of logins stalls unrelated requests.
/// The decoy path goes through here too: leaving it synchronous would preserve
/// the amplifier on precisely the branch an attacker without a valid account
/// reaches.
async fn verify_password_off_runtime(password: &str, hash: &str) -> bool {
    let password = password.to_owned();
    let hash = hash.to_owned();
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

/// Fixed decoy argon2id hash, derived once at process start with the exact same
/// `Argon2::default()` parameters as real account hashes.
///
/// [`verify_credentials`] verifies the supplied password against this hash on
/// the "unknown account" and "inactive account" branches, spending comparable
/// CPU work to a real verification. This narrows the timing signal an
/// attacker could otherwise use to enumerate valid usernames.
///
/// Best effort only: this equalizes hashing cost, not overall wall-clock
/// time. Branch prediction, CPU caches, and allocator behavior can still leak
/// a smaller signal. It is not a hard timing guarantee.
static DUMMY_PASSWORD_HASH: LazyLock<String> =
    LazyLock::new(|| hash_password(&generate_token()).unwrap_or_default());

/// Generates a 32-byte URL-safe random token encoded as hex (64 chars).
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

/// Converts Unix epoch seconds to an RFC3339 UTC string (reuses clock logic).
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
type EnvRow = (String, String, Option<String>, String, String);
type FlagRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
);

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
    metadata_json: &str,
) -> StoreResult<Environment> {
    Ok(Environment {
        key: EnvironmentKey::new(k).map_err(|e| domain_key_err(&e))?,
        name,
        external_ref: ext_ref.map(ExternalRef::new),
        managed_by: managed_by_from_str(mb)?,
        metadata: serde_json::from_str(metadata_json)?,
    })
}

// ---------------------------------------------------------------------------
// Generic read helpers (pool and &mut Transaction both implement Executor)
// ---------------------------------------------------------------------------

async fn do_get_project<'e, E>(executor: E, key: &ProjectKey) -> StoreResult<Option<Project>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row: Option<ProjectRow> = sqlx::query_as(
        "SELECT key, name, description, external_ref, managed_by FROM projects WHERE key = ?",
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
    E: Executor<'e, Database = Sqlite>,
{
    let row: Option<EnvRow> = sqlx::query_as(
        "SELECT key, name, external_ref, managed_by, metadata_json FROM environments WHERE project_key = ? AND key = ?",
    )
    .bind(project.as_str())
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, ext_ref, mb, meta)| row_to_environment(k, name, ext_ref, &mb, &meta))
        .transpose()
}

async fn do_get_flag<'e, E>(
    executor: E,
    project: &ProjectKey,
    key: &FlagKey,
) -> StoreResult<Option<Flag>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row: Option<FlagRow> = sqlx::query_as(
        "SELECT key, name, description, flag_type, value_type, variants_json, metadata_json FROM flags WHERE project_key = ? AND key = ?",
    )
    .bind(project.as_str())
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, desc, ft, vt, vj, mj)| {
        Ok(Flag {
            key: FlagKey::new(k).map_err(|e| domain_key_err(&e))?,
            name,
            description: desc,
            flag_type: serde_json::from_str(&format!(r#""{ft}""#))?,
            value_type: serde_json::from_str(&format!(r#""{vt}""#))?,
            variants: serde_json::from_str(&vj)?,
            metadata: serde_json::from_str(&mj)?,
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
    E: Executor<'e, Database = Sqlite>,
{
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT key, name, match_json FROM segments WHERE project_key = ? AND key = ?",
    )
    .bind(project.as_str())
    .bind(key.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(k, name, mj)| {
        Ok(Segment {
            key: SegmentKey::new(k).map_err(|e| domain_key_err(&e))?,
            name,
            match_expr: serde_json::from_str(&mj)?,
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
    E: Executor<'e, Database = Sqlite>,
{
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT config_json FROM flag_env_configs WHERE project_key = ? AND flag_key = ? AND environment_key = ?",
    )
    .bind(project.as_str())
    .bind(flag.as_str())
    .bind(environment.as_str())
    .fetch_optional(executor)
    .await?;

    row.map(|(cj,)| Ok(serde_json::from_str(&cj)?)).transpose()
}

// ---------------------------------------------------------------------------
// Generic write helpers (pool and &mut Transaction both implement Executor)
// ---------------------------------------------------------------------------

async fn do_upsert_project<'e, E>(executor: E, project: &Project) -> StoreResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let external_ref = project.external_ref.as_ref().map(|r| r.as_str().to_owned());
    let managed_by = managed_by_str(project.managed_by);
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO projects (key, name, description, external_ref, managed_by, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(key) DO UPDATE SET
              name         = excluded.name,
              description  = excluded.description,
              external_ref = excluded.external_ref,
              managed_by   = excluded.managed_by,
              updated_at   = excluded.updated_at",
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
    E: Executor<'e, Database = Sqlite>,
{
    let external_ref = env.external_ref.as_ref().map(|r| r.as_str().to_owned());
    let managed_by = managed_by_str(env.managed_by);
    let metadata_json = serde_json::to_string(&env.metadata)?;
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO environments (project_key, key, name, external_ref, managed_by, metadata_json, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name          = excluded.name,
              external_ref  = excluded.external_ref,
              managed_by    = excluded.managed_by,
              metadata_json = excluded.metadata_json,
              updated_at    = excluded.updated_at",
    )
    .bind(project.as_str())
    .bind(env.key.as_str())
    .bind(&env.name)
    .bind(external_ref)
    .bind(managed_by)
    .bind(&metadata_json)
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
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            Err(StoreError::ForeignKeyViolation)
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

async fn do_upsert_flag<'e, E>(executor: E, project: &ProjectKey, flag: &Flag) -> StoreResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let variants_json = serde_json::to_string(&flag.variants)?;
    let flag_type = serde_json::to_string(&flag.flag_type)?;
    let value_type = serde_json::to_string(&flag.value_type)?;
    let metadata_json = serde_json::to_string(&flag.metadata)?;
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO flags (project_key, key, name, description, flag_type, value_type, variants_json, metadata_json, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name          = excluded.name,
              description   = excluded.description,
              flag_type     = excluded.flag_type,
              value_type    = excluded.value_type,
              variants_json = excluded.variants_json,
              metadata_json = excluded.metadata_json,
              updated_at    = excluded.updated_at",
    )
    .bind(project.as_str())
    .bind(flag.key.as_str())
    .bind(&flag.name)
    .bind(flag.description.as_deref())
    .bind(flag_type.trim_matches('"'))
    .bind(value_type.trim_matches('"'))
    .bind(&variants_json)
    .bind(&metadata_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            Err(StoreError::ForeignKeyViolation)
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

async fn do_upsert_segment<'e, E>(
    executor: E,
    project: &ProjectKey,
    segment: &Segment,
) -> StoreResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let match_json = serde_json::to_string(&segment.match_expr)?;
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO segments (project_key, key, name, match_json, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?)
          ON CONFLICT(project_key, key) DO UPDATE SET
              name       = excluded.name,
              match_json = excluded.match_json,
              updated_at = excluded.updated_at",
    )
    .bind(project.as_str())
    .bind(segment.key.as_str())
    .bind(&segment.name)
    .bind(&match_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            Err(StoreError::ForeignKeyViolation)
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

async fn do_upsert_flag_env_config<'e, E>(
    executor: E,
    project: &ProjectKey,
    flag: &FlagKey,
    environment: &EnvironmentKey,
    config: &FlagEnvConfig,
) -> StoreResult<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let config_json = serde_json::to_string(config)?;
    let now = crate::clock::now_rfc3339();

    let result = sqlx::query(
        r"INSERT INTO flag_env_configs (project_key, flag_key, environment_key, config_json, created_at, updated_at)
          VALUES (?, ?, ?, ?, ?, ?)
          ON CONFLICT(project_key, flag_key, environment_key) DO UPDATE SET
              config_json = excluded.config_json,
              updated_at  = excluded.updated_at",
    )
    .bind(project.as_str())
    .bind(flag.as_str())
    .bind(environment.as_str())
    .bind(&config_json)
    .bind(&now)
    .bind(&now)
    .execute(executor)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
            Err(StoreError::ForeignKeyViolation)
        }
        Err(e) => Err(StoreError::Sqlx(e)),
    }
}

// ---------------------------------------------------------------------------
// Embedded migrations
// ---------------------------------------------------------------------------

/// Returns a [`Migrator`] with the SQLite schema embedded at compile time.
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
                Cow::Borrowed(include_str!("../../migrations/sqlite/0001_init.sql")),
                false,
            ),
            Migration::new(
                2,
                Cow::Borrowed("audit_log"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!("../../migrations/sqlite/0002_audit_log.sql")),
                false,
            ),
            Migration::new(
                3,
                Cow::Borrowed("accounts"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!("../../migrations/sqlite/0003_accounts.sql")),
                false,
            ),
            Migration::new(
                4,
                Cow::Borrowed("sdk_key_revocation"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!(
                    "../../migrations/sqlite/0004_sdk_key_revocation.sql"
                )),
                false,
            ),
            Migration::new(
                5,
                Cow::Borrowed("add_metadata"),
                MigrationType::Simple,
                Cow::Borrowed(include_str!(
                    "../../migrations/sqlite/0005_add_metadata.sql"
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
// SqliteStore
// ---------------------------------------------------------------------------

/// SQLite-backed store: connection pool + HMAC hasher.
#[derive(Clone)]
pub struct SqliteStore {
    pool: Pool<Sqlite>,
    hasher: KeyHasher,
    sdk_key_lookups: Arc<AtomicU64>,
}

impl SqliteStore {
    /// Connects to the given SQLite URL, enables foreign keys, and runs migrations.
    ///
    /// # Errors
    /// Returns [`StoreError`] if the connection or migrations fail.
    pub async fn connect(url: &str, hasher: KeyHasher) -> StoreResult<Self> {
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(url)?.create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await?;
        embedded_migrator().run(&pool).await?;
        Ok(Self {
            pool,
            hasher,
            sdk_key_lookups: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Creates an in-memory SQLite store suitable for tests.
    ///
    /// # Errors
    /// Returns [`StoreError`] if setup or migrations fail.
    pub async fn in_memory(hasher: KeyHasher) -> StoreResult<Self> {
        // Use a named shared cache so all connections in the pool share the
        // same in-memory database.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await?;
        embedded_migrator().run(&pool).await?;
        Ok(Self {
            pool,
            hasher,
            sdk_key_lookups: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Returns the cumulative number of SDK key lookups this store has served.
    ///
    /// Exposed as an operational counter: it is what proves a flood of
    /// impossible credentials is refused before reaching the database.
    #[must_use]
    pub fn sdk_key_lookups(&self) -> u64 {
        self.sdk_key_lookups.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// ProjectRepository for SqliteStore
// ---------------------------------------------------------------------------

impl ProjectRepository for SqliteStore {
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
        sqlx::query("DELETE FROM projects WHERE key = ?")
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
// EnvironmentRepository for SqliteStore
// ---------------------------------------------------------------------------

impl EnvironmentRepository for SqliteStore {
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
            "SELECT key, name, external_ref, managed_by, metadata_json FROM environments WHERE project_key = ?",
        )
        .bind(project.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(k, name, ext_ref, mb, meta)| row_to_environment(k, name, ext_ref, &mb, &meta))
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
        sqlx::query("DELETE FROM environments WHERE project_key = ? AND key = ?")
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
// FlagRepository for SqliteStore
// ---------------------------------------------------------------------------

impl FlagRepository for SqliteStore {
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
        let rows: Vec<FlagRow> = sqlx::query_as(
            "SELECT key, name, description, flag_type, value_type, variants_json, metadata_json FROM flags WHERE project_key = ?",
        )
        .bind(project.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(k, name, desc, ft, vt, vj, mj)| {
                Ok(Flag {
                    key: FlagKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    description: desc,
                    flag_type: serde_json::from_str(&format!(r#""{ft}""#))?,
                    value_type: serde_json::from_str(&format!(r#""{vt}""#))?,
                    variants: serde_json::from_str(&vj)?,
                    metadata: serde_json::from_str(&mj)?,
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
        sqlx::query("DELETE FROM flags WHERE project_key = ? AND key = ?")
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
// SegmentRepository for SqliteStore
// ---------------------------------------------------------------------------

impl SegmentRepository for SqliteStore {
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
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT key, name, match_json FROM segments WHERE project_key = ?")
                .bind(project.as_str())
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|(k, name, mj)| {
                Ok(Segment {
                    key: SegmentKey::new(k).map_err(|e| domain_key_err(&e))?,
                    name,
                    match_expr: serde_json::from_str(&mj)?,
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
        sqlx::query("DELETE FROM segments WHERE project_key = ? AND key = ?")
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
// FlagEnvConfigRepository for SqliteStore
// ---------------------------------------------------------------------------

impl FlagEnvConfigRepository for SqliteStore {
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
            "DELETE FROM flag_env_configs WHERE project_key = ? AND flag_key = ? AND environment_key = ?",
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
// SdkKeyRepository for SqliteStore
// ---------------------------------------------------------------------------

impl SdkKeyRepository for SqliteStore {
    async fn create_sdk_key(
        &self,
        actor: &str,
        raw_key: &str,
        new_key: &NewSdkKey,
    ) -> StoreResult<SdkKeyRecord> {
        let key_hash = self.hasher.hash(raw_key);
        let prefix = raw_key.chars().take(12).collect::<String>();
        let kind_str = serde_json::to_string(&new_key.kind)?;
        let kind_str = kind_str.trim_matches('"');
        let now = crate::clock::now_rfc3339();

        let mut tx = self.pool.begin().await?;

        let insert_result = sqlx::query(
            r"INSERT INTO sdk_keys (key_hash, prefix, kind, project_key, environment_key, created_at)
              VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&key_hash)
        .bind(&prefix)
        .bind(kind_str)
        .bind(new_key.scope.project_key.as_str())
        .bind(new_key.scope.environment_key.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await;

        match insert_result {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                return Err(StoreError::ForeignKeyViolation);
            }
            Err(e) => return Err(StoreError::Sqlx(e)),
        }

        // Audit the issuance, symmetrically to revocation, in the same transaction.
        let entity_id = format!(
            "{}/{}/{}",
            new_key.scope.project_key.as_str(),
            new_key.scope.environment_key.as_str(),
            prefix
        );
        let audit = AuditRecord {
            actor: actor.to_owned(),
            action: "sdk_key.issued".to_owned(),
            entity_type: "sdk_key".to_owned(),
            entity_id,
            before: None,
            after: None,
            occurred_at: now.clone(),
        };
        append_audit(&mut *tx, &audit).await?;

        tx.commit().await?;

        Ok(SdkKeyRecord {
            prefix,
            kind: new_key.kind,
            scope: new_key.scope.clone(),
            created_at: now,
            revoked_at: None,
        })
    }

    async fn find_sdk_key(&self, raw_key: &str) -> StoreResult<Option<SdkKeyRecord>> {
        self.sdk_key_lookups.fetch_add(1, Ordering::Relaxed);

        let key_hash = self.hasher.hash(raw_key);

        let row: Option<(String, String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT prefix, kind, project_key, environment_key, created_at, revoked_at \
             FROM sdk_keys WHERE key_hash = ? AND revoked_at IS NULL",
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        row.map(
            |(prefix, kind_str, proj_key, env_key, created_at, revoked_at)| {
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
                    revoked_at,
                })
            },
        )
        .transpose()
    }

    async fn list_sdk_keys(
        &self,
        _actor: &str,
        scope: &SdkKeyScope,
    ) -> StoreResult<Vec<SdkKeyRecord>> {
        let rows: Vec<(String, String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT prefix, kind, project_key, environment_key, created_at, revoked_at \
             FROM sdk_keys \
             WHERE project_key = ? AND environment_key = ? \
             ORDER BY created_at ASC",
        )
        .bind(scope.project_key.as_str())
        .bind(scope.environment_key.as_str())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(
                |(prefix, kind_str, proj_key, env_key, created_at, revoked_at)| {
                    let kind = serde_json::from_str(&format!(r#""{kind_str}""#))?;
                    Ok(SdkKeyRecord {
                        prefix,
                        kind,
                        scope: SdkKeyScope {
                            project_key: ProjectKey::new(proj_key)
                                .map_err(|e| domain_key_err(&e))?,
                            environment_key: EnvironmentKey::new(env_key)
                                .map_err(|e| domain_key_err(&e))?,
                        },
                        created_at,
                        revoked_at,
                    })
                },
            )
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
            "UPDATE sdk_keys SET revoked_at = ? \
             WHERE project_key = ? AND environment_key = ? AND prefix = ? AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(project.as_str())
        .bind(environment.as_str())
        .bind(prefix)
        .execute(&self.pool)
        .await?;

        // Audit the revocation.
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
// AuditLogRepository for SqliteStore
// ---------------------------------------------------------------------------

type AuditRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
);

fn row_to_audit_record(
    actor: String,
    action: String,
    entity_type: String,
    entity_id: String,
    before_json: Option<String>,
    after_json: Option<String>,
    occurred_at: String,
) -> StoreResult<AuditRecord> {
    let before = before_json
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(StoreError::Serialization)?;
    let after = after_json
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(StoreError::Serialization)?;
    Ok(AuditRecord {
        actor,
        action,
        entity_type,
        entity_id,
        before,
        after,
        occurred_at,
    })
}

impl AuditLogRepository for SqliteStore {
    async fn list_audit_entries(&self) -> StoreResult<Vec<AuditRecord>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT actor, action, entity_type, entity_id, before_json, after_json, occurred_at \
             FROM audit_log ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
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
            .collect()
    }

    async fn audit_entries_for(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> StoreResult<Vec<AuditRecord>> {
        let rows: Vec<AuditRow> = sqlx::query_as(
            "SELECT actor, action, entity_type, entity_id, before_json, after_json, occurred_at \
             FROM audit_log WHERE entity_type = ? AND entity_id = ? ORDER BY id ASC",
        )
        .bind(entity_type)
        .bind(entity_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
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
            .collect()
    }
}

// ---------------------------------------------------------------------------
// AccountRepository for SqliteStore
// ---------------------------------------------------------------------------

impl AccountRepository for SqliteStore {
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
              VALUES (?, ?, ?, 1, ?)",
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
        let row: Option<(String, String, String, i64)> = sqlx::query_as(
            "SELECT id, username, password_hash, is_active FROM accounts WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        let Some((id, uname, hash, is_active)) = row else {
            // Spend the same argon2 verification cost as a real account so that
            // enumeration cannot be inferred from response timing (best effort).
            let _ = verify_password_off_runtime(password, &DUMMY_PASSWORD_HASH).await;
            return Ok(None);
        };

        if is_active == 0 {
            let _ = verify_password_off_runtime(password, &DUMMY_PASSWORD_HASH).await;
            return Ok(None);
        }

        if verify_password_off_runtime(password, &hash).await {
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
// SessionRepository for SqliteStore
// ---------------------------------------------------------------------------

impl SessionRepository for SqliteStore {
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
              VALUES (?, ?, ?)",
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
             WHERE s.token_hash = ? \
               AND s.revoked_at IS NULL \
               AND s.expires_at > ? \
               AND a.is_active = 1",
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
            "UPDATE sessions SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL",
        )
        .bind(&now)
        .bind(&token_hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TransactionalStore for SqliteStore
// ---------------------------------------------------------------------------

/// A write session bound to a single SQLite transaction.
pub struct SqliteWriteSession<'a> {
    tx: Transaction<'a, Sqlite>,
    actor: String,
}

impl TransactionalStore for SqliteStore {
    type Session<'a> = SqliteWriteSession<'a>;

    async fn begin(&self, actor: &str) -> StoreResult<Self::Session<'_>> {
        let tx = self.pool.begin().await?;
        Ok(SqliteWriteSession {
            tx,
            actor: actor.to_owned(),
        })
    }
}

impl WriteSession for SqliteWriteSession<'_> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use crate::hash::KeyHasher;

    /// Issue #98 regression: `SqliteStore::connect` must create the database
    /// file when it does not exist yet, matching a fresh Docker volume or a
    /// first run of the daemon. Before the fix, the pool opens the URL with
    /// the default `create_if_missing = false`, so connecting to a file that
    /// has never been created fails with SQLite error 14 (unable to open
    /// database file) and the daemon never boots.
    #[tokio::test]
    async fn connect_creates_missing_database_file() {
        let dir = std::env::temp_dir().join(format!("flaps-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("failed to create the temp test directory");
        let db_path = dir.join("flaps.db");
        assert!(
            !db_path.exists(),
            "test setup invariant: the database file must not exist yet"
        );

        let url = format!(
            "sqlite:{}",
            db_path.display().to_string().replace('\\', "/")
        );

        let result = SqliteStore::connect(&url, KeyHasher::new(b"test-pepper".to_vec())).await;

        assert!(
            result.is_ok(),
            "connect must succeed and create the database file on a fresh path: {:?}",
            result.err()
        );
        assert!(
            db_path.exists(),
            "the database file must exist on disk after connect"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
