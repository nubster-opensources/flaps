//! Audit record type and the internal append helper.
//!
//! [`AuditRecord`] describes one successful mutation. Records are written by the
//! store itself; no public API allows forging or modifying them.

/// An immutable audit record describing one successful mutation.
///
/// `before`/`after` are JSON snapshots of the aggregate. `before` is `None` for
/// a creation, `after` is `None` for a deletion. The store mints these records;
/// there is no public API to forge one.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    /// Principal who performed the action (free-form: user id, service account).
    pub actor: String,
    /// Derived machine-readable action, e.g. `"flag.updated"`.
    pub action: String,
    /// Aggregate kind, e.g. `"project"`, `"flag_env_config"`.
    pub entity_type: String,
    /// Slash-joined key path, e.g. `"my-project/my-flag/prod"`.
    pub entity_id: String,
    /// JSON snapshot before the mutation (`None` on creation).
    pub before: Option<serde_json::Value>,
    /// JSON snapshot after the mutation (`None` on deletion).
    pub after: Option<serde_json::Value>,
    /// RFC3339 UTC timestamp, minted via `clock::now_rfc3339`.
    pub occurred_at: String,
}

// ---------------------------------------------------------------------------
// SQLite append helper
// ---------------------------------------------------------------------------

pub(crate) mod sqlite {
    use sqlx::{Executor, Sqlite};

    use crate::error::{StoreError, StoreResult};

    use super::AuditRecord;

    /// Appends one audit record inside an existing SQLite transaction (or pool).
    ///
    /// Strictly `pub(crate)`: callers outside this crate cannot write audit entries.
    pub(crate) async fn append_audit<'e, E>(executor: E, record: &AuditRecord) -> StoreResult<()>
    where
        E: Executor<'e, Database = Sqlite>,
    {
        let before_json = record
            .before
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(StoreError::Serialization)?;
        let after_json = record
            .after
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(StoreError::Serialization)?;

        sqlx::query(
            r"INSERT INTO audit_log (actor, action, entity_type, entity_id, before_json, after_json, occurred_at)
              VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&record.actor)
        .bind(&record.action)
        .bind(&record.entity_type)
        .bind(&record.entity_id)
        .bind(before_json)
        .bind(after_json)
        .bind(&record.occurred_at)
        .execute(executor)
        .await
        .map_err(StoreError::Sqlx)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL append helper
// ---------------------------------------------------------------------------

pub(crate) mod postgres {
    use sqlx::{Executor, Postgres};

    use crate::error::{StoreError, StoreResult};

    use super::AuditRecord;

    /// Appends one audit record inside an existing PostgreSQL transaction (or pool).
    ///
    /// Strictly `pub(crate)`: callers outside this crate cannot write audit entries.
    pub(crate) async fn append_audit<'e, E>(executor: E, record: &AuditRecord) -> StoreResult<()>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let before_json: Option<serde_json::Value> = record.before.clone();
        let after_json: Option<serde_json::Value> = record.after.clone();

        sqlx::query(
            r"INSERT INTO audit_log (actor, action, entity_type, entity_id, before_json, after_json, occurred_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&record.actor)
        .bind(&record.action)
        .bind(&record.entity_type)
        .bind(&record.entity_id)
        .bind(before_json)
        .bind(after_json)
        .bind(&record.occurred_at)
        .execute(executor)
        .await
        .map_err(StoreError::Sqlx)?;

        Ok(())
    }
}
