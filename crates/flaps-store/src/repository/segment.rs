//! Repository trait for the Segment aggregate.

use std::future::Future;

use flaps_domain::{ProjectKey, Segment, SegmentKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Segment`] aggregates scoped to a project.
pub trait SegmentRepository: Send + Sync {
    /// Inserts or fully replaces the segment within `project`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    fn upsert_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        segment: &Segment,
    ) -> impl Future<Output = StoreResult<()>> + Send;

    /// Returns the segment for `key` within `project`, or `None`.
    fn get_segment(
        &self,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> impl Future<Output = StoreResult<Option<Segment>>> + Send;

    /// Returns all segments for `project` in insertion order.
    fn list_segments(
        &self,
        project: &ProjectKey,
    ) -> impl Future<Output = StoreResult<Vec<Segment>>> + Send;

    /// Deletes the segment identified by `project` + `key`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the segment does not exist this is a no-op and no
    /// audit entry is written.
    fn delete_segment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &SegmentKey,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
