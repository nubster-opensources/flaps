//! Project and organizational types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a tenant (maps to Workspace Organization).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub Uuid);

impl TenantId {
    /// Creates a new random tenant ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a tenant ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(pub Uuid);

impl GroupId {
    /// Creates a new random group ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a group ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for GroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GroupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    /// Creates a new random project ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a project ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An optional organizational group within a tenant.
///
/// Groups provide an intermediate level of organization between
/// tenants and projects. They are optional - projects can be
/// directly under a tenant without belonging to a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Unique identifier.
    pub id: GroupId,
    /// Machine-readable key (e.g., "client-a", "mobile-team").
    pub key: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Tenant this group belongs to.
    pub tenant_id: TenantId,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl Group {
    /// Creates a new group.
    pub fn new(key: impl Into<String>, name: impl Into<String>, tenant_id: TenantId) -> Self {
        Self {
            id: GroupId::new(),
            key: key.into(),
            name: name.into(),
            description: None,
            tenant_id,
            created_at: Utc::now(),
        }
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// A project containing flags and environments.
///
/// Projects are the main organizational unit for flags.
/// Each project has its own set of environments and flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Unique identifier.
    pub id: ProjectId,
    /// Machine-readable key (e.g., "backend-api", "mobile-app").
    pub key: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Optional group this project belongs to.
    pub group_id: Option<GroupId>,
    /// Tenant this project belongs to.
    pub tenant_id: TenantId,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

impl Project {
    /// Creates a new project.
    pub fn new(key: impl Into<String>, name: impl Into<String>, tenant_id: TenantId) -> Self {
        let now = Utc::now();
        Self {
            id: ProjectId::new(),
            key: key.into(),
            name: name.into(),
            description: None,
            group_id: None,
            tenant_id,
            created_at: now,
            updated_at: now,
        }
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the group.
    pub fn with_group(mut self, group_id: GroupId) -> Self {
        self.group_id = Some(group_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_project() {
        let tenant_id = TenantId::new();
        let project = Project::new("backend-api", "Backend API", tenant_id)
            .with_description("Main backend service");

        assert_eq!(project.key, "backend-api");
        assert_eq!(project.name, "Backend API");
        assert!(project.description.is_some());
        assert!(project.group_id.is_none());
    }

    #[test]
    fn test_create_project_with_group() {
        let tenant_id = TenantId::new();
        let group = Group::new("client-a", "Client A", tenant_id);
        let project = Project::new("client-a-api", "Client A API", tenant_id).with_group(group.id);

        assert_eq!(project.group_id, Some(group.id));
    }
}
