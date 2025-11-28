//! HTTP client for the Workspace API.

use flaps_core::{Project, ProjectId, TenantId};

use crate::error::{StorageError, StorageResult};
use crate::traits::WorkspaceClient;

/// Configuration for the Workspace API client.
#[derive(Debug, Clone)]
pub struct WorkspaceClientConfig {
    /// Base URL of the Workspace API.
    pub base_url: String,
    /// API key or token for authentication.
    pub api_key: Option<String>,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for WorkspaceClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:8080".to_string(),
            api_key: None,
            timeout_secs: 30,
        }
    }
}

/// HTTP-based implementation of the Workspace client.
#[derive(Debug, Clone)]
pub struct HttpWorkspaceClient {
    config: WorkspaceClientConfig,
    client: reqwest::Client,
}

impl HttpWorkspaceClient {
    /// Creates a new HTTP Workspace client.
    pub fn new(config: WorkspaceClientConfig) -> StorageResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| {
                StorageError::Configuration(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self { config, client })
    }

    /// Creates a client with default configuration.
    pub fn with_base_url(base_url: impl Into<String>) -> StorageResult<Self> {
        Self::new(WorkspaceClientConfig {
            base_url: base_url.into(),
            ..Default::default()
        })
    }
}

impl WorkspaceClient for HttpWorkspaceClient {
    async fn get_project(&self, id: ProjectId) -> StorageResult<Option<Project>> {
        let url = format!("{}/api/v1/projects/{}", self.config.base_url, id.0);

        let mut request = self.client.get(&url);
        if let Some(ref api_key) = self.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| StorageError::Configuration(format!("Workspace API error: {}", e)))?;

        match response.status() {
            reqwest::StatusCode::OK => {
                let project = response.json::<Project>().await.map_err(|e| {
                    StorageError::Serialization(serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        e.to_string(),
                    )))
                })?;
                Ok(Some(project))
            },
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(StorageError::Configuration(format!(
                "Workspace API returned status {}: {}",
                status,
                response.text().await.unwrap_or_default()
            ))),
        }
    }

    async fn list_projects(&self, tenant_id: TenantId) -> StorageResult<Vec<Project>> {
        let url = format!(
            "{}/api/v1/tenants/{}/projects",
            self.config.base_url, tenant_id.0
        );

        let mut request = self.client.get(&url);
        if let Some(ref api_key) = self.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| StorageError::Configuration(format!("Workspace API error: {}", e)))?;

        if response.status().is_success() {
            let projects = response.json::<Vec<Project>>().await.map_err(|e| {
                StorageError::Serialization(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )))
            })?;
            Ok(projects)
        } else {
            Err(StorageError::Configuration(format!(
                "Workspace API returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )))
        }
    }

    async fn validate_project_access(
        &self,
        tenant_id: TenantId,
        project_id: ProjectId,
    ) -> StorageResult<bool> {
        match self.get_project(project_id).await? {
            Some(project) => Ok(project.tenant_id == tenant_id),
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = WorkspaceClientConfig::default();
        assert_eq!(config.base_url, "http://localhost:8080");
        assert!(config.api_key.is_none());
        assert_eq!(config.timeout_secs, 30);
    }
}
