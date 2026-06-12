//! Repository trait for the Segment aggregate.

use flaps_domain::{ProjectKey, Segment, SegmentKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Segment`] aggregates scoped to a project.
#[allow(async_fn_in_trait)]
pub trait SegmentRepository {
    /// Inserts or fully replaces the segment within `project`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    async fn upsert_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        segment: &Segment,
    ) -> StoreResult<()>;

    /// Returns the segment for `key` within `project`, or `None`.
    async fn get_segment(
        &self,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> StoreResult<Option<Segment>>;

    /// Returns all segments for `project` in insertion order.
    async fn list_segments(&self, project: &ProjectKey) -> StoreResult<Vec<Segment>>;

    /// Deletes the segment identified by `project` + `key`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the segment does not exist this is a no-op and no
    /// audit entry is written.
    async fn delete_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> StoreResult<()>;
}
