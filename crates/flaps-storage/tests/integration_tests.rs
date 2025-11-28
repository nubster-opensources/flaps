//! Integration tests for flaps-storage.
//!
//! These tests use SQLite in-memory for fast, isolated testing.

use flaps_core::{Environment, Flag, FlagKey, ProjectId, Segment, UserId};
use flaps_storage::{
    Database, DatabaseConfig, EnvironmentRepository, FlagRepository, SegmentRepository,
    SqliteRepositories,
};

/// Creates a test database with the schema applied.
async fn setup_test_db() -> SqliteRepositories {
    let config = DatabaseConfig::sqlite_memory();
    let db = Database::connect(&config)
        .await
        .expect("Failed to connect to SQLite");

    let pool = db.sqlite().expect("Expected SQLite pool");

    // Apply the schema
    sqlx::query(include_str!(
        "../migrations/20250128_001_initial_schema.sql"
    ))
    .execute(pool)
    .await
    .expect("Failed to apply schema");

    SqliteRepositories::new(pool.clone())
}

mod flag_repository_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_flag() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let flag = Flag::new_boolean("test-flag", "Test Flag", project_id, UserId::new("user-1"));

        // Create
        repos
            .flags
            .create(&flag)
            .await
            .expect("Failed to create flag");

        // Get by ID
        let retrieved = repos
            .flags
            .get_by_id(flag.id)
            .await
            .expect("Failed to get flag")
            .expect("Flag not found");

        assert_eq!(retrieved.id, flag.id);
        assert_eq!(retrieved.key.as_str(), "test-flag");
        assert_eq!(retrieved.name, "Test Flag");
    }

    #[tokio::test]
    async fn test_get_flag_by_key() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let flag = Flag::new_boolean(
            "my-feature",
            "My Feature",
            project_id,
            UserId::new("user-1"),
        );
        repos
            .flags
            .create(&flag)
            .await
            .expect("Failed to create flag");

        // Get by key
        let key = FlagKey::new("my-feature");
        let retrieved = repos
            .flags
            .get_by_key(project_id, &key)
            .await
            .expect("Failed to get flag")
            .expect("Flag not found");

        assert_eq!(retrieved.id, flag.id);
    }

    #[tokio::test]
    async fn test_list_flags_by_project() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        // Create multiple flags
        let flag1 = Flag::new_boolean("flag-a", "Flag A", project_id, UserId::new("user-1"));
        let flag2 = Flag::new_boolean("flag-b", "Flag B", project_id, UserId::new("user-1"));
        let flag3 = Flag::new_boolean("flag-c", "Flag C", project_id, UserId::new("user-1"));

        repos
            .flags
            .create(&flag1)
            .await
            .expect("Failed to create flag1");
        repos
            .flags
            .create(&flag2)
            .await
            .expect("Failed to create flag2");
        repos
            .flags
            .create(&flag3)
            .await
            .expect("Failed to create flag3");

        // List
        let flags = repos
            .flags
            .list_by_project(project_id)
            .await
            .expect("Failed to list flags");
        assert_eq!(flags.len(), 3);

        // Should be ordered by name
        assert_eq!(flags[0].name, "Flag A");
        assert_eq!(flags[1].name, "Flag B");
        assert_eq!(flags[2].name, "Flag C");
    }

    #[tokio::test]
    async fn test_update_flag() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let mut flag = Flag::new_boolean(
            "update-me",
            "Original Name",
            project_id,
            UserId::new("user-1"),
        );
        repos
            .flags
            .create(&flag)
            .await
            .expect("Failed to create flag");

        // Update
        flag.name = "Updated Name".to_string();
        flag.description = Some("New description".to_string());
        repos
            .flags
            .update(&flag)
            .await
            .expect("Failed to update flag");

        // Verify
        let retrieved = repos
            .flags
            .get_by_id(flag.id)
            .await
            .expect("Failed to get flag")
            .expect("Flag not found");
        assert_eq!(retrieved.name, "Updated Name");
        assert_eq!(retrieved.description, Some("New description".to_string()));
    }

    #[tokio::test]
    async fn test_delete_flag() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let flag = Flag::new_boolean("delete-me", "Delete Me", project_id, UserId::new("user-1"));
        repos
            .flags
            .create(&flag)
            .await
            .expect("Failed to create flag");

        // Delete
        repos
            .flags
            .delete(flag.id)
            .await
            .expect("Failed to delete flag");

        // Verify deleted
        let result = repos
            .flags
            .get_by_id(flag.id)
            .await
            .expect("Failed to get flag");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_duplicate_flag_key_fails() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let flag1 = Flag::new_boolean("same-key", "Flag 1", project_id, UserId::new("user-1"));
        let flag2 = Flag::new_boolean("same-key", "Flag 2", project_id, UserId::new("user-1"));

        repos
            .flags
            .create(&flag1)
            .await
            .expect("Failed to create flag1");

        // Should fail
        let result = repos.flags.create(&flag2).await;
        assert!(result.is_err());
    }
}

mod segment_repository_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_segment() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let segment = Segment::new(
            "beta-users",
            "Beta Users",
            project_id,
            UserId::new("user-1"),
        )
        .with_description("Beta testing users")
        .with_included_user("user-123")
        .with_excluded_user("banned-user");

        repos
            .segments
            .create(&segment)
            .await
            .expect("Failed to create segment");

        let retrieved = repos
            .segments
            .get_by_id(segment.id)
            .await
            .expect("Failed to get segment")
            .expect("Segment not found");

        assert_eq!(retrieved.id, segment.id);
        assert_eq!(retrieved.key, "beta-users");
        assert_eq!(retrieved.included_users, vec!["user-123"]);
        assert_eq!(retrieved.excluded_users, vec!["banned-user"]);
    }

    #[tokio::test]
    async fn test_list_segments_by_project() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let segment1 = Segment::new("segment-a", "Segment A", project_id, UserId::new("user-1"));
        let segment2 = Segment::new("segment-b", "Segment B", project_id, UserId::new("user-1"));

        repos
            .segments
            .create(&segment1)
            .await
            .expect("Failed to create segment1");
        repos
            .segments
            .create(&segment2)
            .await
            .expect("Failed to create segment2");

        let segments = repos
            .segments
            .list_by_project(project_id)
            .await
            .expect("Failed to list segments");
        assert_eq!(segments.len(), 2);
    }
}

mod environment_repository_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_environment() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        let env = Environment::production(project_id);

        repos
            .environments
            .create(&env)
            .await
            .expect("Failed to create environment");

        let retrieved = repos
            .environments
            .get_by_id(env.id)
            .await
            .expect("Failed to get environment")
            .expect("Environment not found");

        assert_eq!(retrieved.id, env.id);
        assert_eq!(retrieved.key, "prod");
        assert!(retrieved.is_production);
    }

    #[tokio::test]
    async fn test_list_environments_ordered() {
        let repos = setup_test_db().await;
        let project_id = ProjectId::new();

        // Create environments in random order
        let prod = Environment::production(project_id);
        let dev = Environment::development(project_id);
        let staging = Environment::staging(project_id);

        repos
            .environments
            .create(&prod)
            .await
            .expect("Failed to create prod");
        repos
            .environments
            .create(&dev)
            .await
            .expect("Failed to create dev");
        repos
            .environments
            .create(&staging)
            .await
            .expect("Failed to create staging");

        // Should be ordered by sort_order
        let envs = repos
            .environments
            .list_by_project(project_id)
            .await
            .expect("Failed to list environments");
        assert_eq!(envs.len(), 3);
        assert_eq!(envs[0].key, "dev"); // order: 0
        assert_eq!(envs[1].key, "staging"); // order: 1
        assert_eq!(envs[2].key, "prod"); // order: 2
    }
}
