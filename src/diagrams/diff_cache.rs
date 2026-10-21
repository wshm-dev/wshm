//! Small in-memory LRU of PR diffs for the daemon-hosted diagram pages: the
//! page is rendered on request, the diff comes from the forge, and this cache
//! keeps repeated views from refetching it. Bounded in entries and bytes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A cached diff: `None` when it was larger than the page cap (only the
/// verdict is kept, so a huge diff is not refetched on every view).
pub type CachedDiff = Option<Arc<str>>;

pub struct DiffCache {
    max_entries: usize,
    max_bytes: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, (CachedDiff, u64)>,
    bytes: usize,
    tick: u64,
}

fn size(v: &CachedDiff) -> usize {
    v.as_ref().map_or(0, |s| s.len())
}

impl DiffCache {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn get(&self, key: &str) -> Option<CachedDiff> {
        let mut g = self.inner.lock().ok()?;
        g.tick += 1;
        let tick = g.tick;
        let e = g.map.get_mut(key)?;
        e.1 = tick;
        Some(e.0.clone())
    }

    pub fn put(&self, key: String, value: CachedDiff) {
        if size(&value) > self.max_bytes || self.max_entries == 0 {
            return;
        }
        let Ok(mut g) = self.inner.lock() else {
            return;
        };
        g.tick += 1;
        let tick = g.tick;
        let added = size(&value);
        if let Some((old, _)) = g.map.insert(key, (value, tick)) {
            g.bytes -= size(&old);
        }
        g.bytes += added;
        while g.map.len() > self.max_entries || g.bytes > self.max_bytes {
            let Some(oldest) = g
                .map
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((v, _)) = g.map.remove(&oldest) {
                g.bytes -= size(&v);
            }
        }
    }

    #[cfg(test)]
    fn stats(&self) -> (usize, usize) {
        let g = self.inner.lock().unwrap();
        (g.map.len(), g.bytes)
    }
}

/// Process-wide cache: 64 diffs, 32 MB.
pub fn global() -> &'static DiffCache {
    static C: OnceLock<DiffCache> = OnceLock::new();
    C.get_or_init(|| DiffCache::new(64, 32 * 1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> CachedDiff {
        Some(Arc::from(s))
    }

    #[test]
    fn evicts_least_recently_used_by_entries() {
        let c = DiffCache::new(2, 1000);
        c.put("a".into(), v("1"));
        c.put("b".into(), v("2"));
        assert!(c.get("a").is_some()); // a is now more recent than b
        c.put("c".into(), v("3"));
        assert!(c.get("b").is_none());
        assert!(c.get("a").is_some() && c.get("c").is_some());
        assert_eq!(c.stats(), (2, 2));
    }

    #[test]
    fn bounded_in_bytes() {
        let c = DiffCache::new(10, 10);
        c.put("a".into(), v("123456"));
        c.put("b".into(), v("123456"));
        assert_eq!(c.stats(), (1, 6), "a evicted to stay under 10 bytes");
        c.put("huge".into(), v("12345678901"));
        assert!(c.get("huge").is_none(), "bigger than the whole cache");
        c.put("b".into(), v("12"));
        assert_eq!(c.stats(), (1, 2), "replacing an entry fixes the byte count");
    }

    #[test]
    fn remembers_too_large_verdicts() {
        let c = DiffCache::new(2, 10);
        c.put("x".into(), None);
        assert_eq!(c.get("x"), Some(None));
        assert_eq!(c.get("y"), None);
    }
}
