//! Integration tests for the PostgreSQL backend.
//!
//! This suite runs only when `FLAPS_TEST_POSTGRES_URL` is set.
//! If the variable is absent (local development without a Postgres instance),
//! the test returns immediately without failing.

mod shared;

use flaps_store::{KeyHasher, postgres::PostgresStore};

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
