use std::{
    collections::{BTreeMap, HashMap, HashSet},
    hash::Hash,
};

/// A cache that holds values up to a total size in bytes and evicts the least
/// recently used ones beyond it.
///
/// Pinned values are never evicted, so the values held can exceed the
/// capacity while the pinned ones alone do.
#[derive(Debug)]
pub struct LruCache<K, V> {
    entries: HashMap<K, CacheEntry<V>>,
    /// The keys by the time they were last used, the oldest first.
    by_use: BTreeMap<u64, K>,
    /// Keys whose values are not evicted. They need not be held.
    pinned: HashSet<K>,
    bytes: usize,
    capacity: usize,
    /// Increases on every use.
    clock: u64,
}

#[derive(Debug)]
struct CacheEntry<V> {
    value: V,
    bytes: usize,
    last_used: u64,
}

impl<K: Clone + Eq + Hash, V> LruCache<K, V> {
    /// Creates a cache that holds up to `capacity` bytes.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            by_use: BTreeMap::new(),
            pinned: HashSet::new(),
            bytes: 0,
            capacity,
            clock: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The total size of the values held.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[must_use]
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Returns the value and marks it as the most recently used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let entry = self.entries.get_mut(key)?;
        self.by_use.remove(&entry.last_used);
        self.clock += 1;
        entry.last_used = self.clock;
        self.by_use.insert(self.clock, key.clone());
        Some(&entry.value)
    }

    /// Returns the value without marking it as used.
    #[must_use]
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.entries.get(key).map(|entry| &entry.value)
    }

    /// Adds `value`, `bytes` in size, as the most recently used, replacing the
    /// value of `key` if any.
    ///
    /// Returns the values that no longer fit, the least recently used first,
    /// so that the caller can release them. A value that does not fit even
    /// with only the pinned values left is returned at once, unless it is
    /// pinned itself.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) -> Vec<(K, V)> {
        let mut evicted = Vec::new();
        if let Some(old) = self.remove(&key) {
            evicted.push((key.clone(), old));
        }
        if !self.pinned.contains(&key) && self.pinned_bytes() + bytes > self.capacity {
            evicted.push((key, value));
            return evicted;
        }
        self.evict_to_fit(bytes, &mut evicted);
        self.clock += 1;
        self.by_use.insert(self.clock, key.clone());
        self.entries.insert(
            key,
            CacheEntry {
                value,
                bytes,
                last_used: self.clock,
            },
        );
        self.bytes += bytes;
        evicted
    }

    /// Changes the total size the cache holds.
    ///
    /// Returns the values that no longer fit, the least recently used first.
    pub fn set_capacity(&mut self, capacity: usize) -> Vec<(K, V)> {
        self.capacity = capacity;
        let mut evicted = Vec::new();
        self.evict_to_fit(0, &mut evicted);
        evicted
    }

    /// Replaces the pinned keys.
    ///
    /// Returns the values that no longer fit once the previous ones are no
    /// longer pinned, the least recently used first.
    pub fn set_pinned(&mut self, pinned: HashSet<K>) -> Vec<(K, V)> {
        self.pinned = pinned;
        let mut evicted = Vec::new();
        self.evict_to_fit(0, &mut evicted);
        evicted
    }

    /// The total size of the pinned values held.
    fn pinned_bytes(&self) -> usize {
        self.pinned
            .iter()
            .filter_map(|key| self.entries.get(key))
            .map(|entry| entry.bytes)
            .sum()
    }

    /// Evicts the least recently used values that are not pinned until
    /// `extra` more bytes fit, or no such value is left.
    fn evict_to_fit(&mut self, extra: usize, evicted: &mut Vec<(K, V)>) {
        while self.bytes + extra > self.capacity {
            let Some(oldest) = self
                .by_use
                .values()
                .find(|key| !self.pinned.contains(*key))
                .cloned()
            else {
                break;
            };
            if let Some(value) = self.remove(&oldest) {
                evicted.push((oldest, value));
            }
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let entry = self.entries.remove(key)?;
        self.by_use.remove(&entry.last_used);
        self.bytes -= entry.bytes;
        Some(entry.value)
    }

    /// Removes all values and returns them, so that the caller can release them.
    pub fn drain(&mut self) -> Vec<(K, V)> {
        self.by_use.clear();
        self.bytes = 0;
        self.entries
            .drain()
            .map(|(key, entry)| (key, entry.value))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(evicted: &[(&'static str, u32)]) -> Vec<&'static str> {
        evicted.iter().map(|(key, _)| *key).collect()
    }

    #[test]
    fn insert_and_get() {
        let mut cache = LruCache::new(10);
        assert_eq!(cache.insert("a", 1, 4), []);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.get(&"b"), None);
        assert_eq!((cache.len(), cache.bytes()), (1, 4));
    }

    #[test]
    fn evicts_least_recently_inserted() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        let evicted = cache.insert("c", 3, 4);
        assert_eq!(keys(&evicted), ["a"]);
        assert!(!cache.contains(&"a"));
        assert_eq!(cache.bytes(), 8);
    }

    #[test]
    fn peek_does_not_keep_value_from_eviction() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.peek(&"a"), Some(&1));
        let evicted = cache.insert("c", 3, 4);
        assert_eq!(keys(&evicted), ["a"]);
    }

    #[test]
    fn get_keeps_value_from_eviction() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        cache.get(&"a");
        let evicted = cache.insert("c", 3, 4);
        assert_eq!(keys(&evicted), ["b"]);
        assert!(cache.contains(&"a"));
    }

    #[test]
    fn evicts_as_many_as_needed() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 3);
        cache.insert("b", 2, 3);
        cache.insert("c", 3, 3);
        let evicted = cache.insert("d", 4, 8);
        assert_eq!(keys(&evicted), ["a", "b", "c"]);
        assert_eq!((cache.len(), cache.bytes()), (1, 8));
    }

    #[test]
    fn insert_replaces_existing_value() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        let evicted = cache.insert("a", 2, 6);
        assert_eq!(evicted, [("a", 1)]);
        assert_eq!(cache.get(&"a"), Some(&2));
        assert_eq!(cache.bytes(), 6);
    }

    #[test]
    fn value_larger_than_capacity_is_not_held() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        let evicted = cache.insert("big", 2, 11);
        assert_eq!(evicted, [("big", 2)]);
        assert!(cache.contains(&"a"));
    }

    fn pin(keys: &[&'static str]) -> HashSet<&'static str> {
        keys.iter().copied().collect()
    }

    #[test]
    fn pinned_value_is_not_evicted() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.set_pinned(pin(&["a"])), []);
        let evicted = cache.insert("c", 3, 4);
        assert_eq!(keys(&evicted), ["b"]);
        assert!(cache.contains(&"a"));
    }

    #[test]
    fn pinned_values_can_exceed_capacity() {
        let mut cache = LruCache::new(10);
        cache.set_pinned(pin(&["a", "b", "c"]));
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.insert("c", 3, 4), []);
        assert_eq!(cache.bytes(), 12);
        // An unpinned value does not fit next to them.
        let evicted = cache.insert("d", 4, 1);
        assert_eq!(keys(&evicted), ["d"]);
    }

    #[test]
    fn pinned_value_larger_than_capacity_is_held() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.set_pinned(pin(&["big"]));
        let evicted = cache.insert("big", 2, 11);
        assert_eq!(keys(&evicted), ["a"]);
        assert_eq!(cache.get(&"big"), Some(&2));
    }

    #[test]
    fn unpinning_evicts_what_no_longer_fits() {
        let mut cache = LruCache::new(10);
        cache.set_pinned(pin(&["a", "b", "c"]));
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        cache.insert("c", 3, 4);
        cache.get(&"a");
        let evicted = cache.set_pinned(pin(&["c"]));
        assert_eq!(keys(&evicted), ["b"]);
        assert_eq!(cache.bytes(), 8);
    }

    #[test]
    fn smaller_capacity_evicts_least_recently_used() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        cache.get(&"a");
        assert_eq!(keys(&cache.set_capacity(5)), ["b"]);
        assert_eq!(cache.insert("c", 3, 2), [("a", 1)]);
        assert_eq!(cache.set_capacity(100), []);
    }

    #[test]
    fn remove_and_drain() {
        let mut cache = LruCache::new(10);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.remove(&"a"), Some(1));
        assert_eq!(cache.remove(&"a"), None);
        assert_eq!(cache.bytes(), 4);
        assert_eq!(cache.drain(), [("b", 2)]);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.insert("c", 3, 10), []);
    }
}
