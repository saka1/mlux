use std::collections::HashMap;

use log::trace;
use serde::{Deserialize, Serialize};

use super::tile::TilePairHash;

/// A pair of rendered PNGs: content + sidebar for the same tile index.
#[derive(Debug, Serialize, Deserialize)]
pub struct TilePngs {
    pub content: Vec<u8>,
    pub sidebar: Vec<u8>,
}

/// Cache for rendered tile PNGs, separated from [`super::tile::TiledDocument`]
/// to allow concurrent `&TiledDocument` access (e.g., from a prefetch worker
/// thread) while the main thread owns `&mut TileCache`.
struct TiledDocumentCache {
    data: HashMap<usize, TilePngs>,
}

impl TiledDocumentCache {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    fn get(&self, idx: usize) -> Option<&TilePngs> {
        self.data.get(&idx)
    }

    fn contains(&self, idx: usize) -> bool {
        self.data.contains_key(&idx)
    }

    fn insert(&mut self, idx: usize, pngs: TilePngs) {
        self.data.insert(idx, pngs);
    }

    /// Evict entries far from `center`, keeping only those within `keep_radius`.
    fn evict_distant(&mut self, center: usize, keep_radius: usize) {
        let to_evict: Vec<usize> = self
            .data
            .keys()
            .filter(|&&k| (k as isize - center as isize).unsigned_abs() > keep_radius)
            .copied()
            .collect();
        for k in to_evict {
            self.data.remove(&k);
            trace!("cache evict tile {}", k);
        }
    }

    fn remove(&mut self, idx: usize) -> Option<TilePngs> {
        self.data.remove(&idx)
    }

    fn len(&self) -> usize {
        self.data.len()
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn clear(&mut self) {
        self.data.clear();
    }
}

/// Composite cache that pairs rendered tile PNGs with their content hashes.
///
/// Encapsulates `TiledDocumentCache` and the hash vector that were previously
/// managed as separate `Option` fields in the viewer outer loop.
pub struct TileCache {
    cache: TiledDocumentCache,
    hashes: Vec<TilePairHash>,
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
            cache: TiledDocumentCache::new(),
            hashes: Vec::new(),
        }
    }

    /// Merge cached tile PNGs from a previous generation into a new cache.
    ///
    /// For each tile in the new document, if the old cache has a tile with the
    /// same content hash, move the PNG over. Returns the number of recovered
    /// tiles.
    pub fn merge_generation(&mut self, new_hashes: &[TilePairHash]) -> usize {
        let mut new_cache = TiledDocumentCache::new();
        for (new_idx, new_hash) in new_hashes.iter().enumerate() {
            if let Some(old_idx) = self.hashes.iter().position(|h| h == new_hash)
                && let Some(pngs) = self.cache.remove(old_idx)
            {
                new_cache.insert(new_idx, pngs);
            }
        }
        let recovered = new_cache.len();
        self.cache = new_cache;
        self.hashes = new_hashes.to_vec();
        recovered
    }

    /// Discard all cached data (e.g., when navigating to a different document).
    pub fn clear(&mut self) {
        self.cache.clear();
        self.hashes.clear();
    }

    pub fn get(&self, idx: usize) -> Option<&TilePngs> {
        self.cache.get(idx)
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.cache.contains(idx)
    }

    pub fn insert(&mut self, idx: usize, pngs: TilePngs) {
        self.cache.insert(idx, pngs);
    }

    /// Evict entries far from `center`, keeping only those within `keep_radius`.
    pub fn evict_distant(&mut self, center: usize, keep_radius: usize) {
        self.cache.evict_distant(center, keep_radius);
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
    use crate::frame::TileHash;

    fn make_hash(v: u8) -> TilePairHash {
        TilePairHash {
            content: TileHash::new_for_test(v as u64),
            sidebar: TileHash::new_for_test(v as u64),
        }
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

        let recovered = tc.merge_generation(&hashes);
        assert_eq!(recovered, 2);
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

        let recovered = tc.merge_generation(&new_hashes);
        assert_eq!(recovered, 1);
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

        let recovered = tc.merge_generation(&new_hashes);
        assert_eq!(recovered, 0);
    }

    #[test]
    fn merge_generation_evicted_not_recovered() {
        let hashes = vec![make_hash(1)];
        let mut tc = TileCache::new();
        tc.hashes = hashes.clone();
        // Don't insert anything — simulates evicted tile (no PNG in cache)

        let recovered = tc.merge_generation(&hashes);
        assert_eq!(recovered, 0);
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
