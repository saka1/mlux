use std::collections::HashMap;

use log::trace;

use super::tile::{TileHash, TilePngs};

/// Result of [`TileCache::merge_generation`].
pub struct MergeResult {
    /// Tiles with a matching hash whose cached PNGs were recovered.
    pub recovered: usize,
    /// Tiles whose content hash matched the previous generation (including
    /// those without cached PNGs, e.g. never rendered or evicted).
    pub hash_matched: usize,
    /// Total number of tiles in the new generation.
    pub total: usize,
}

/// Composite cache that pairs rendered tile PNGs with their content hashes.
pub struct TileCache {
    cache: HashMap<usize, TilePngs>,
    hashes: Vec<TileHash>,
}

impl Default for TileCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TileCache {
    /// Create an empty tile cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            hashes: Vec::new(),
        }
    }

    /// Merge cached tile PNGs from a previous generation into a new cache.
    ///
    /// For each tile in the new document, if the old cache has a tile with the
    /// same content hash, move the PNG over.
    pub fn merge_generation(&mut self, new_hashes: &[TileHash]) -> MergeResult {
        let mut new_cache = HashMap::new();
        let mut hash_matched = 0usize;
        for (new_idx, new_hash) in new_hashes.iter().enumerate() {
            if let Some(old_idx) = self.hashes.iter().position(|h| h == new_hash) {
                hash_matched += 1;
                if let Some(pngs) = self.cache.remove(&old_idx) {
                    new_cache.insert(new_idx, pngs);
                }
            }
        }
        let recovered = new_cache.len();
        let total = new_hashes.len();
        self.cache = new_cache;
        self.hashes = new_hashes.to_vec();
        MergeResult {
            recovered,
            hash_matched,
            total,
        }
    }

    /// Discard all cached data (e.g., when navigating to a different document).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.hashes.clear();
    }

    pub fn get(&self, idx: usize) -> Option<&TilePngs> {
        self.cache.get(&idx)
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.cache.contains_key(&idx)
    }

    pub fn insert(&mut self, idx: usize, pngs: TilePngs) {
        self.cache.insert(idx, pngs);
    }

    /// Evict entries far from `center`, keeping only those within `keep_radius`.
    pub fn evict_distant(&mut self, center: usize, keep_radius: usize) {
        let to_evict: Vec<usize> = self
            .cache
            .keys()
            .filter(|&&k| (k as isize - center as isize).unsigned_abs() > keep_radius)
            .copied()
            .collect();
        for k in to_evict {
            self.cache.remove(&k);
            trace!("cache evict tile {}", k);
        }
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_hash(v: u8) -> TileHash {
        let mut arr = [0u8; 32];
        arr[0] = v;
        TileHash::new_for_test(arr)
    }

    fn make_pngs(tag: u8) -> TilePngs {
        TilePngs {
            content: vec![tag],
            sidebar: vec![tag],
        }
    }

    #[test]
    fn merge_generation_full_match() {
        let hashes = vec![make_hash(1), make_hash(2)];
        let mut tc = TileCache::new();
        // Seed the cache with old generation
        tc.hashes = hashes.clone();
        tc.cache.insert(0, make_pngs(10));
        tc.cache.insert(1, make_pngs(20));

        let m = tc.merge_generation(&hashes);
        assert_eq!(m.recovered, 2);
        assert_eq!(m.hash_matched, 2);
        assert_eq!(m.total, 2);
        assert_eq!(tc.get(0).unwrap().content, vec![10]);
        assert_eq!(tc.get(1).unwrap().content, vec![20]);
    }

    #[test]
    fn merge_generation_partial_match() {
        let old_hashes = vec![make_hash(1), make_hash(2)];
        let new_hashes = vec![make_hash(1), make_hash(3)];
        let mut tc = TileCache::new();
        tc.hashes = old_hashes;
        tc.cache.insert(0, make_pngs(10));
        tc.cache.insert(1, make_pngs(20));

        let m = tc.merge_generation(&new_hashes);
        assert_eq!(m.recovered, 1);
        assert_eq!(m.hash_matched, 1);
        assert_eq!(m.total, 2);
        assert!(tc.contains(0));
        assert!(!tc.contains(1));
    }

    #[test]
    fn merge_generation_zero_match() {
        let old_hashes = vec![make_hash(1), make_hash(2)];
        let new_hashes = vec![make_hash(3), make_hash(4)];
        let mut tc = TileCache::new();
        tc.hashes = old_hashes;
        tc.cache.insert(0, make_pngs(10));
        tc.cache.insert(1, make_pngs(20));

        let m = tc.merge_generation(&new_hashes);
        assert_eq!(m.recovered, 0);
        assert_eq!(m.hash_matched, 0);
        assert_eq!(m.total, 2);
    }

    #[test]
    fn merge_generation_evicted_not_recovered() {
        let hashes = vec![make_hash(1)];
        let mut tc = TileCache::new();
        tc.hashes = hashes.clone();
        // Don't insert anything — simulates evicted tile (no PNG in cache)

        let m = tc.merge_generation(&hashes);
        assert_eq!(m.recovered, 0);
        assert_eq!(m.hash_matched, 1);
        assert_eq!(m.total, 1);
    }

    #[test]
    fn clear_empties_both() {
        let mut tc = TileCache::new();
        tc.hashes = vec![make_hash(1)];
        tc.cache.insert(0, make_pngs(10));

        tc.clear();
        assert!(tc.is_empty());
        assert_eq!(tc.len(), 0);
    }
}
