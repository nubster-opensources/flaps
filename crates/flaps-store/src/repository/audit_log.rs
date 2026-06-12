//! Repository trait for reading the append-only audit log.
//!
//! Writing is intentionally excluded: audit entries are created exclusively by
//! the internal `append_audit` helper, which is invoked within the same
//! transaction as each mutation.

use crate::{audit::AuditRecord, error::StoreResult};

/// Read-only access to the append-only audit log.
///
/// No update or delete method exists. Entries are ordered chronologically
/// (oldest first) by `occurred_at`.
#[allow(async_fn_in_trait)]
pub trait AuditLogRepository {
    /// Returns all audit records, oldest first.
    async fn list_audit_entries(&self) -> StoreResult<Vec<AuditRecord>>;

    /// Returns audit records for one entity (identified by `entity_type` and
    /// `entity_id`), oldest first.
    async fn audit_entries_for(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> StoreResult<Vec<AuditRecord>>;
}
