//! Repository trait for SDK key persistence.

use crate::error::StoreResult;
use crate::sdk_key::{NewSdkKey, SdkKeyRecord};

/// Async operations for persisting and looking up SDK keys.
///
/// Raw key values are hashed before storage; this trait never returns
/// a raw key or its HMAC hash.
#[allow(async_fn_in_trait)]
pub trait SdkKeyRepository {
    /// Hashes `raw_key` with the store's [`KeyHasher`](crate::KeyHasher), derives the
    /// readable prefix (the leading 12 characters of `raw_key`, or the whole value if
    /// shorter), and persists the secret-free record.
    async fn create_sdk_key(&self, raw_key: &str, new_key: &NewSdkKey)
    -> StoreResult<SdkKeyRecord>;

    /// Hashes `raw_key` and looks the record up by hash.
    ///
    /// Returns `None` if no key with a matching hash exists.
    async fn find_sdk_key(&self, raw_key: &str) -> StoreResult<Option<SdkKeyRecord>>;
}
