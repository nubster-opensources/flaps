//! Storage traits for Flaps.

use std::future::Future;

use flaps_core::{
    Environment, Flag, FlagId, FlagKey, Group, GroupId, Project, ProjectId,
    Result, Segment, SegmentId, TenantId,
};

/// Repository for flag operations.
pub trait FlagRepository: Send + Sync {
    /// Gets a flag by ID.
    fn get_by_id(&self, id: FlagId) -> impl Future<Output = Result<Option<Flag>>> + Send;

    /// Gets a flag by key within a project.
    fn get_by_key(&self, project_id: ProjectId, key: &FlagKey) -> impl Future<Output = Result<Option<Flag>>> + Send;

    /// Lists all flags in a project.
    fn list_by_project(&self, project_id: ProjectId) -> impl Future<Output = Result<Vec<Flag>>> + Send;

    /// Creates a new flag.
    fn create(&self, flag: &Flag) -> impl Future<Output = Result<()>> + Send;

    /// Updates an existing flag.
    fn update(&self, flag: &Flag) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a flag.
    fn delete(&self, id: FlagId) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for segment operations.
pub trait SegmentRepository: Send + Sync {
    /// Gets a segment by ID.
    fn get_by_id(&self, id: SegmentId) -> impl Future<Output = Result<Option<Segment>>> + Send;

    /// Lists all segments in a project.
    fn list_by_project(&self, project_id: ProjectId) -> impl Future<Output = Result<Vec<Segment>>> + Send;

    /// Creates a new segment.
    fn create(&self, segment: &Segment) -> impl Future<Output = Result<()>> + Send;

    /// Updates an existing segment.
    fn update(&self, segment: &Segment) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a segment.
    fn delete(&self, id: SegmentId) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for project operations.
pub trait ProjectRepository: Send + Sync {
    /// Gets a project by ID.
    fn get_by_id(&self, id: ProjectId) -> impl Future<Output = Result<Option<Project>>> + Send;

    /// Lists all projects in a tenant.
    fn list_by_tenant(&self, tenant_id: TenantId) -> impl Future<Output = Result<Vec<Project>>> + Send;

    /// Lists all projects in a group.
    fn list_by_group(&self, group_id: GroupId) -> impl Future<Output = Result<Vec<Project>>> + Send;

    /// Creates a new project.
    fn create(&self, project: &Project) -> impl Future<Output = Result<()>> + Send;

    /// Updates an existing project.
    fn update(&self, project: &Project) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a project.
    fn delete(&self, id: ProjectId) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for environment operations.
pub trait EnvironmentRepository: Send + Sync {
    /// Lists all environments in a project.
    fn list_by_project(&self, project_id: ProjectId) -> impl Future<Output = Result<Vec<Environment>>> + Send;

    /// Creates a new environment.
    fn create(&self, environment: &Environment) -> impl Future<Output = Result<()>> + Send;

    /// Updates an existing environment.
    fn update(&self, environment: &Environment) -> impl Future<Output = Result<()>> + Send;

    /// Deletes an environment.
    fn delete(&self, id: flaps_core::EnvironmentId) -> impl Future<Output = Result<()>> + Send;
}

/// Repository for group operations.
pub trait GroupRepository: Send + Sync {
    /// Gets a group by ID.
    fn get_by_id(&self, id: GroupId) -> impl Future<Output = Result<Option<Group>>> + Send;

    /// Lists all groups in a tenant.
    fn list_by_tenant(&self, tenant_id: TenantId) -> impl Future<Output = Result<Vec<Group>>> + Send;

    /// Creates a new group.
    fn create(&self, group: &Group) -> impl Future<Output = Result<()>> + Send;

    /// Updates an existing group.
    fn update(&self, group: &Group) -> impl Future<Output = Result<()>> + Send;

    /// Deletes a group.
    fn delete(&self, id: GroupId) -> impl Future<Output = Result<()>> + Send;
}

/// Cache for flag configurations.
pub trait FlagCache: Send + Sync {
    /// Gets cached flags for a project/environment.
    fn get(&self, project_id: ProjectId, environment: &str) -> impl Future<Output = Result<Option<Vec<Flag>>>> + Send;

    /// Sets cached flags for a project/environment.
    fn set(&self, project_id: ProjectId, environment: &str, flags: &[Flag]) -> impl Future<Output = Result<()>> + Send;

    /// Invalidates cache for a project/environment.
    fn invalidate(&self, project_id: ProjectId, environment: Option<&str>) -> impl Future<Output = Result<()>> + Send;
}
