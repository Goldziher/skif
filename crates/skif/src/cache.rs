//! The cache store a host gives the linter (ADR 0007): the [`CacheStore`] trait, its namespaces, keys and envelope,
//! the counters a run reports in [`Report::cache`](crate::Report::cache), a store in memory and, with `cache-disk`,
//! skif's own disk store.

/// skif's disk store: per-base stores under one location, pruned to what the last run used and capped in size.
#[cfg(feature = "cache-disk")]
pub use skif_cache::{
    CacheStats, CleanReport, DEFAULT_MAX_SIZE, DiskStore, DiskStoreOptions, Finish, NamespaceStats, StoreError,
    StoreStats, base_key, clean, default_location, stats,
};
pub use skif_core::cache::{
    CACHE_VERSION, CacheCounters, CacheHandle, CacheNamespace, CacheStore, KeyBuilder, MemoryStore, content_digest,
    open, salt_digest, seal,
};
/// The namespace of parse results and the module cache served from a store.
#[cfg(feature = "js-analysis")]
pub use skif_js::{MODULES, StoreModuleCache};
