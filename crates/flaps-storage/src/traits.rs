//! Storage traits for Flaps.

use std::future::Future;

use flaps_core::{
    Environment, EnvironmentId, Flag, FlagId, FlagKey, Project, ProjectId, Segment, SegmentId,
    TenantId,
};

use crate::error::StorageResult;

// =============================================================================
// Workspace Integration (External API)
// =============================================================================

/// Client for interacting with the Nubster Workspace API.
///
/// Projects, tenants, and groups are managed by the Workspace service.
/// Flaps fetches this data via the Workspace API rather than storing it locally.
pub trait WorkspaceClient: Send + Sync {
    /// Gets a project by ID from Workspace.
    fn get_project(
        &self,
        id: ProjectId,
    ) -> impl Future<Output = StorageResult<Option<Project>>> + Send;

    /// Lists all projects accessible to the current tenant.
    fn list_projects(
        &self,
        tenant_id: TenantId,
    ) -> impl Future<Output = StorageResult<Vec<Project>>> + Send;

    /// Validates that a project exists and belongs to the tenant.
    fn validate_project_access(
        &self,
        tenant_id: TenantId,
        project_id: ProjectId,
    ) -> impl Future<Output = StorageResult<bool>> + Send;
}

// =============================================================================
// Local Repositories (Flaps-specific data)
// =============================================================================

/// Repository for flag operations.
pub trait FlagRepository: Send + Sync {
    /// Gets a flag by ID.
    fn get_by_id(&self, id: FlagId) -> impl Future<Output = StorageResult<Option<Flag>>> + Send;

    /// Gets a flag by key within a project.
    fn get_by_key(
        &self,
        project_id: ProjectId,
        key: &FlagKey,
    ) -> impl Future<Output = StorageResult<Option<Flag>>> + Send;

    /// Lists all flags in a project.
    fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> impl Future<Output = StorageResult<Vec<Flag>>> + Send;

    /// Creates a new flag.
    fn create(&self, flag: &Flag) -> impl Future<Output = StorageResult<()>> + Send;

    /// Updates an existing flag.
    fn update(&self, flag: &Flag) -> impl Future<Output = StorageResult<()>> + Send;

    /// Deletes a flag.
    fn delete(&self, id: FlagId) -> impl Future<Output = StorageResult<()>> + Send;
}

/// Repository for segment operations.
pub trait SegmentRepository: Send + Sync {
    /// Gets a segment by ID.
    fn get_by_id(
        &self,
        id: SegmentId,
    ) -> impl Future<Output = StorageResult<Option<Segment>>> + Send;

    /// Gets a segment by key within a project.
    fn get_by_key(
        &self,
        project_id: ProjectId,
        key: &str,
    ) -> impl Future<Output = StorageResult<Option<Segment>>> + Send;

    /// Lists all segments in a project.
    fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> impl Future<Output = StorageResult<Vec<Segment>>> + Send;

    /// Creates a new segment.
    fn create(&self, segment: &Segment) -> impl Future<Output = StorageResult<()>> + Send;

    /// Updates an existing segment.
    fn update(&self, segment: &Segment) -> impl Future<Output = StorageResult<()>> + Send;

    /// Deletes a segment.
    fn delete(&self, id: SegmentId) -> impl Future<Output = StorageResult<()>> + Send;
}

/// Repository for environment operations.
pub trait EnvironmentRepository: Send + Sync {
    /// Gets an environment by ID.
    fn get_by_id(
        &self,
        id: EnvironmentId,
    ) -> impl Future<Output = StorageResult<Option<Environment>>> + Send;

    /// Gets an environment by key within a project.
    fn get_by_key(
        &self,
        project_id: ProjectId,
        key: &str,
    ) -> impl Future<Output = StorageResult<Option<Environment>>> + Send;

    /// Lists all environments in a project.
    fn list_by_project(
        &self,
        project_id: ProjectId,
    ) -> impl Future<Output = StorageResult<Vec<Environment>>> + Send;

    /// Creates a new environment.
    fn create(&self, environment: &Environment) -> impl Future<Output = StorageResult<()>> + Send;

    /// Updates an existing environment.
    fn update(&self, environment: &Environment) -> impl Future<Output = StorageResult<()>> + Send;

    /// Deletes an environment.
    fn delete(&self, id: EnvironmentId) -> impl Future<Output = StorageResult<()>> + Send;
}

// =============================================================================
// Cache Layer
// =============================================================================

/// Cache for flag configurations.
pub trait FlagCache: Send + Sync {
    /// Gets cached flags for a project/environment.
    fn get(
        &self,
        project_id: ProjectId,
        environment: &str,
    ) -> impl Future<Output = StorageResult<Option<Vec<Flag>>>> + Send;

    /// Sets cached flags for a project/environment.
    fn set(
        &self,
        project_id: ProjectId,
        environment: &str,
        flags: &[Flag],
        ttl_secs: u64,
    ) -> impl Future<Output = StorageResult<()>> + Send;

    /// Invalidates cache for a project/environment.
    fn invalidate(
        &self,
        project_id: ProjectId,
        environment: Option<&str>,
    ) -> impl Future<Output = StorageResult<()>> + Send;
}
