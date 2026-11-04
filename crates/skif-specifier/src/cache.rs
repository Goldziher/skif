// Added for skif (not in syncpack): caller-owned replacement for the upstream thread-local cache; see ATTRIBUTIONS.md.
use {
    crate::Specifier,
    rustc_hash::{FxBuildHasher, FxHashMap},
    std::{
        hash::BuildHasher,
        sync::{Arc, PoisonError, RwLock},
    },
};

/// Independently locked parts of the cache, so parallel member loading rarely waits on one lock.
const SHARDS: usize = 16;

type Shard = RwLock<FxHashMap<Box<str>, Arc<Specifier>>>;

/// A caller-owned, thread-safe cache which deduplicates parsed [`Specifier`]s
/// by their raw string.
#[derive(Debug, Default)]
pub struct SpecifierCache {
    shards: [Shard; SHARDS],
}

impl SpecifierCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn shard(&self, raw: &str) -> &Shard {
        let index = usize::try_from(FxBuildHasher.hash_one(raw) % SHARDS as u64).unwrap_or(0);
        &self.shards[index]
    }

    /// Return the cached Specifier for `raw`, parsing and storing it on first use.
    ///
    /// # Panics
    ///
    /// Never panics on its own; a poisoned lock is recovered because the cache
    /// only ever holds fully-constructed values.
    #[must_use]
    pub fn get_or_parse(&self, raw: &str) -> Arc<Specifier> {
        let shard = self.shard(raw);
        if let Some(found) = shard.read().unwrap_or_else(PoisonError::into_inner).get(raw) {
            return Arc::clone(found);
        }
        let parsed = Specifier::new(raw);
        let mut entries = shard.write().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(entries.entry(raw.into()).or_insert(parsed))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Specifier>;
    let _ = assert_send_sync::<SpecifierCache>;
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_same_arc_for_the_same_input() {
        let cache = SpecifierCache::new();
        let a = cache.get_or_parse("^1.2.3");
        let b = cache.get_or_parse("^1.2.3");
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn distinct_inputs_get_distinct_entries() {
        let cache = SpecifierCache::new();
        let a = cache.get_or_parse("^1.2.3");
        let b = cache.get_or_parse("~1.2.3");
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(cache.len(), 2);
        assert!(!cache.is_empty());
    }

    #[test]
    fn is_shareable_across_threads() {
        let cache = Arc::new(SpecifierCache::new());
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || cache.get_or_parse("1.0.0"))
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
    }
}
