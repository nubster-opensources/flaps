//! Input types that callers provide to [`crate::compile_environment`].

use std::collections::HashMap;

use flaps_domain::{
    flag::Flag, flag_env_config::FlagEnvConfig, key::SegmentKey, segment::SegmentMatch,
};

/// A feature flag together with its configuration in the environment being compiled.
pub struct FlagConfig<'a> {
    /// The flag definition (variants, value type, key).
    pub flag: &'a Flag,
    /// The per-environment configuration (enabled, rules, default).
    pub config: &'a FlagEnvConfig,
}

/// A lookup table that resolves [`SegmentKey`]s to their match expressions.
///
/// Callers build this from the project's segment list before calling
/// [`crate::compile_environment`]. The compiler inlines each referenced
/// segment directly into the output without emitting `$evaluators` entries.
pub struct Segments<'a> {
    inner: HashMap<SegmentKey, &'a SegmentMatch>,
}

impl<'a> Segments<'a> {
    /// Constructs a [`Segments`] lookup from an iterable of `(key, match_expr)` pairs.
    pub fn new(items: impl IntoIterator<Item = (SegmentKey, &'a SegmentMatch)>) -> Self {
        Self {
            inner: items.into_iter().collect(),
        }
    }

    /// Returns the [`SegmentMatch`] for `key`, or [`None`] when not found.
    #[must_use]
    pub fn get(&self, key: &SegmentKey) -> Option<&'a SegmentMatch> {
        self.inner.get(key).copied()
    }
}
