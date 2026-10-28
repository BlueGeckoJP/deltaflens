use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
};

/// A cache that holds values up to a total size in bytes and evicts the least
/// recently used ones beyond it.
#[derive(Debug)]
pub struct LruCache<K, V> {
    entries: HashMap<K, CacheEntry<V>>,
    /// The keys by the time they were last used, the oldest first.
    by_use: BTreeMap<u64, K>,
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

    /// Adds `value`, `bytes` in size, as the most recently used, replacing the
    /// value of `key` if any.
    ///
    /// Returns the values that no longer fit, the least recently used first,
    /// so that the caller can release them. A value larger than the capacity is
    /// returned at once.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) -> Vec<(K, V)> {
        let mut evicted = Vec::new();
        if let Some(old) = self.remove(&key) {
            evicted.push((key.clone(), old));
        }
        if bytes > self.capacity {
            evicted.push((key, value));
            return evicted;
        }
        while self.bytes + bytes > self.capacity {
            let Some((_, oldest)) = self.by_use.pop_first() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.bytes;
                evicted.push((oldest, entry.value));
            }
        }
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
