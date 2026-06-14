//! Repository trait for SDK key persistence.

use std::future::Future;

use flaps_domain::{EnvironmentKey, ProjectKey};

use crate::error::StoreResult;
use crate::sdk_key::{NewSdkKey, SdkKeyRecord, SdkKeyScope};

/// Async operations for persisting and looking up SDK keys.
///
/// Raw key values are hashed before storage; this trait never returns
/// a raw key or its HMAC hash.
pub trait SdkKeyRepository: Send + Sync {
    /// Hashes `raw_key` with the store's [`KeyHasher`](crate::KeyHasher), derives the
    /// readable prefix (the leading 12 characters of `raw_key`, or the whole value if
    /// shorter), and persists the secret-free record.
    fn create_sdk_key(
        &self,
        raw_key: &str,
        new_key: &NewSdkKey,
    ) -> impl Future<Output = StoreResult<SdkKeyRecord>> + Send;

    /// Hashes `raw_key` and looks the record up by hash.
    ///
    /// Returns `None` if no key with a matching hash exists **or if the key has
    /// been revoked**.
    fn find_sdk_key(
        &self,
        raw_key: &str,
    ) -> impl Future<Output = StoreResult<Option<SdkKeyRecord>>> + Send;

    /// Lists all SDK key records for the given scope (revoked and active alike).
    ///
    /// The returned records never carry the raw key or its hash.
    fn list_sdk_keys(
        &self,
        actor: &str,
        scope: &SdkKeyScope,
    ) -> impl Future<Output = StoreResult<Vec<SdkKeyRecord>>> + Send;

    /// Soft-revokes the key identified by `prefix` in the given scope.
    ///
    /// No-op if the key does not exist or is already revoked.
    fn revoke_sdk_key(
        &self,
        actor: &str,
        project: &ProjectKey,
        environment: &EnvironmentKey,
        prefix: &str,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
