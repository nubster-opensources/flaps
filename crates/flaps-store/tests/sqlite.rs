//! Integration tests for the SQLite backend.

mod shared;

use flaps_domain::{EnvironmentKey, ProjectKey, SdkKeyKind};
use flaps_store::{
    KeyHasher, NewSdkKey, SdkKeyScope,
    repository::{AccountRepository, EnvironmentRepository, ProjectRepository, SdkKeyRepository},
    sqlite::SqliteStore,
};

/// Runs the full shared suite against an in-memory SQLite store.
#[tokio::test]
async fn sqlite_suite() {
    let store = SqliteStore::in_memory(KeyHasher::new(b"test-pepper".to_vec()))
        .await
        .unwrap();
    shared::run_all(store).await;
}

/// Test 10: sdk_key_is_hashed_at_rest.
///
/// Verifies that the prefix stored is the leading portion of the raw key (not the
/// full value), that a second lookup with a different raw key returns None, and
/// that two hashers with different peppers produce different hashes for the same
/// raw key (so the hash is pepper-dependent).
#[tokio::test]
async fn sdk_key_is_hashed_at_rest() {
    let pepper = b"at-rest-pepper".to_vec();
    let store = SqliteStore::in_memory(KeyHasher::new(pepper.clone()))
        .await
        .unwrap();

    let proj = shared::make_project("hash-proj");
    let env = shared::make_env("production");
    store.upsert_project("tester", &proj).await.unwrap();
    store
        .upsert_environment("tester", &proj.key, &env)
        .await
        .unwrap();

    let raw_key = "secret-server-sdk-key-12345";
    let new_key = NewSdkKey {
        kind: SdkKeyKind::Server,
        scope: SdkKeyScope {
            project_key: ProjectKey::new("hash-proj").unwrap(),
            environment_key: EnvironmentKey::new("production").unwrap(),
        },
    };
    let record = store
        .create_sdk_key("tester", raw_key, &new_key)
        .await
        .unwrap();

    // The prefix must equal the first 12 chars of the raw key.
    let expected_prefix: String = raw_key.chars().take(12).collect();
    assert_eq!(
        record.prefix, expected_prefix,
        "stored prefix must be the leading 12 chars of the raw key"
    );

    // The prefix must NOT equal the full raw key (test assumes raw_key.len() > 12).
    assert!(
        raw_key.len() > 12,
        "fixture: raw_key must be longer than 12 chars"
    );
    assert_ne!(
        record.prefix, raw_key,
        "prefix must not equal the full raw value"
    );

    // Lookup succeeds with the correct raw key.
    let found = store.find_sdk_key(raw_key).await.unwrap();
    assert!(
        found.is_some(),
        "lookup by raw key must succeed after create"
    );

    // Lookup with a different raw key must return None.
    let not_found = store.find_sdk_key("wrong-key").await.unwrap();
    assert!(
        not_found.is_none(),
        "lookup with wrong raw key must return None"
    );

    // Verify that the HMAC hasher with a different pepper produces a different hash
    // (the hash stored in the DB is pepper-dependent).
    let hasher_a = KeyHasher::new(pepper.clone());
    let hasher_b = KeyHasher::new(b"other-pepper".to_vec());
    assert_ne!(
        hasher_a.hash(raw_key),
        hasher_b.hash(raw_key),
        "different peppers must produce different hashes"
    );
}

/// Closes the coverage gap left by `test_verify_credentials_inactive_account`
/// in the shared suite (a documented no-op there, since `AccountRepository`
/// exposes no deactivation method). Uses a file-backed SQLite database so a
/// second raw connection can flip `is_active` directly, then asserts
/// `verify_credentials` still returns `None` (#51: this must hold whether the
/// account is unknown, inactive, or the password is wrong; the dummy-hash
/// timing equalization does not change this observable behaviour).
#[tokio::test]
async fn inactive_account_cannot_authenticate() {
    let db_path = std::env::temp_dir().join(format!("flaps-test-{}.sqlite3", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let store = SqliteStore::connect(&url, KeyHasher::new(b"inactive-test-pepper".to_vec()))
        .await
        .unwrap();

    store
        .create_account("system", "dave-inactive", "correct-password")
        .await
        .unwrap();

    // Sanity check: the account authenticates while active.
    let found = store
        .verify_credentials("dave-inactive", "correct-password")
        .await
        .unwrap();
    assert!(found.is_some(), "active account must authenticate");

    // Deactivate directly via a second raw connection to the same file.
    let raw_pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET is_active = 0 WHERE username = ?")
        .bind("dave-inactive")
        .execute(&raw_pool)
        .await
        .unwrap();
    raw_pool.close().await;

    let found = store
        .verify_credentials("dave-inactive", "correct-password")
        .await
        .unwrap();
    assert!(
        found.is_none(),
        "inactive account must return None even with the correct password (anti-enumeration)"
    );

    let _ = std::fs::remove_file(&db_path);
}
