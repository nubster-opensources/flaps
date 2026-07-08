//! Integration tests for the PostgreSQL backend.
//!
//! This suite runs only when `FLAPS_TEST_POSTGRES_URL` is set.
//! If the variable is absent (local development without a Postgres instance),
//! the test returns immediately without failing.

mod shared;

use flaps_store::{KeyHasher, postgres::PostgresStore, repository::AccountRepository};

/// Runs the full shared suite against a PostgreSQL instance.
///
/// Skipped silently when `FLAPS_TEST_POSTGRES_URL` is not set.
#[tokio::test]
async fn postgres_suite() {
    let Ok(url) = std::env::var("FLAPS_TEST_POSTGRES_URL") else {
        return;
    };
    let store = PostgresStore::connect(&url, KeyHasher::new(b"test-pepper".to_vec()))
        .await
        .unwrap();
    shared::run_all(store).await;
}

/// Closes the coverage gap left by `test_verify_credentials_inactive_account`
/// in the shared suite (a documented no-op there, since `AccountRepository`
/// exposes no deactivation method). Uses a second raw connection to flip
/// `is_active` directly, then asserts `verify_credentials` still returns
/// `None` (#51: this must hold whether the account is unknown, inactive, or
/// the password is wrong; the dummy-hash timing equalization does not change
/// this observable behaviour).
///
/// Skipped silently when `FLAPS_TEST_POSTGRES_URL` is not set.
#[tokio::test]
async fn inactive_account_cannot_authenticate() {
    let Ok(url) = std::env::var("FLAPS_TEST_POSTGRES_URL") else {
        return;
    };
    let store = PostgresStore::connect(&url, KeyHasher::new(b"inactive-test-pepper".to_vec()))
        .await
        .unwrap();

    store
        .create_account("system", "dave-inactive-pg", "correct-password")
        .await
        .unwrap();

    let found = store
        .verify_credentials("dave-inactive-pg", "correct-password")
        .await
        .unwrap();
    assert!(found.is_some(), "active account must authenticate");

    let raw_pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("UPDATE accounts SET is_active = false WHERE username = $1")
        .bind("dave-inactive-pg")
        .execute(&raw_pool)
        .await
        .unwrap();
    raw_pool.close().await;

    let found = store
        .verify_credentials("dave-inactive-pg", "correct-password")
        .await
        .unwrap();
    assert!(
        found.is_none(),
        "inactive account must return None even with the correct password (anti-enumeration)"
    );
}
