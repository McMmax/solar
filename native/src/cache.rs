//! Bounded native chunk cache, byte-accurate accounting, LRU eviction, and pinning.

use crate::{
    SolarChunkBase, SolarChunkFrame, SolarChunkHead, SolarChunkPatch, SolarReadError,
    reconstruct_chunk_frame,
};
use solar_schema::unpack_chunk_key;
use std::collections::HashMap;

/// Configuration parameters for native cache limits and eviction thresholds.
#[derive(Clone, Debug)]
pub struct SolarCacheConfig {
    /// Maximum memory budget per world in bytes (default: 64 MB).
    pub max_world_bytes: usize,
    /// Maximum chunk frames cached per world (default: 4096 frames).
    pub max_world_frames: usize,
    /// Global memory budget across all worlds in bytes (default: 256 MB).
    pub max_global_bytes: usize,
    /// Global chunk frame limit across all worlds (default: 16384 frames).
    pub max_global_frames: usize,
}

impl Default for SolarCacheConfig {
    fn default() -> Self {
        Self {
            max_world_bytes: 64 * 1024 * 1024, // 64 MB
            max_world_frames: 4096,
            max_global_bytes: 256 * 1024 * 1024, // 256 MB
            max_global_frames: 16384,
        }
    }
}

/// Explicit metrics for cache operational monitoring.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct SolarCacheMetrics {
    /// Number of cache hits (frame retrieved without reconstruction).
    pub cache_hits: u64,
    /// Number of cache misses (required frame reconstruction or lookup failed).
    pub cache_misses: u64,
    /// Number of successful chunk frame reconstructions.
    pub reconstructions: u64,
    /// Number of frames evicted by LRU policy.
    pub evictions_lru: u64,
    /// Total bytes freed via LRU eviction.
    pub evicted_bytes: u64,
    /// Current cached bytes across all attached worlds.
    pub current_bytes: usize,
    /// Current cached frame count across all attached worlds.
    pub current_frames: usize,
    /// Current count of actively pinned frames (protected from eviction).
    pub pinned_frames: usize,
}

/// Individual cached entry with memory accounting, LRU sequence, and pin refcounting.
#[derive(Clone, Debug)]
pub struct SolarCachedEntry {
    pub frame: SolarChunkFrame,
    pub byte_size: usize,
    pub last_access_seq: u64,
    pub pin_count: u32,
}

impl SolarCachedEntry {
    pub fn new(frame: SolarChunkFrame, access_seq: u64) -> Self {
        let byte_size = frame.approximate_byte_size();
        Self {
            frame,
            byte_size,
            last_access_seq: access_seq,
            pin_count: 0,
        }
    }

    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.pin_count > 0
    }
}

/// World-level subscription and cache state.
#[derive(Default, Debug)]
pub struct SolarWorldCacheState {
    pub world_key: u64,
    /// Cached entries keyed by packed `u128 chunk_key`.
    pub entries: HashMap<u128, SolarCachedEntry>,
    /// Last observed head revisions keyed by packed `u128 chunk_key`.
    pub head_revisions: HashMap<u128, u64>,
    /// Total bytes held in memory by this world's cached frames.
    pub total_bytes: usize,
}

impl SolarWorldCacheState {
    pub fn new(world_key: u64) -> Self {
        Self {
            world_key,
            entries: HashMap::new(),
            head_revisions: HashMap::new(),
            total_bytes: 0,
        }
    }

    /// Inserts or replaces a cached frame, updating byte tracking.
    pub fn insert_entry(
        &mut self,
        chunk_key: u128,
        frame: SolarChunkFrame,
        access_seq: u64,
    ) -> usize {
        let new_entry = SolarCachedEntry::new(frame, access_seq);
        let added_bytes = new_entry.byte_size;

        if let Some(old) = self.entries.insert(chunk_key, new_entry) {
            self.total_bytes = self.total_bytes.saturating_sub(old.byte_size);
        }
        self.total_bytes += added_bytes;
        added_bytes
    }

    /// Removes a cached entry, subtracting its byte size from world accounting.
    pub fn remove_entry(&mut self, chunk_key: &u128) -> Option<SolarCachedEntry> {
        if let Some(entry) = self.entries.remove(chunk_key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.byte_size);
            Some(entry)
        } else {
            None
        }
    }
}

/// Native subscription session and bounded chunk frame cache for attached worlds.
///
/// Features:
/// - Explicit world attach/detach lifecycle with clean subscription management.
/// - Byte-accurate memory accounting per-world and globally.
/// - Deterministic LRU eviction of unpinned chunk frames when limits are exceeded.
/// - Active chunk pinning (`pin_chunk` / `unpin_chunk`) preventing eviction of subscribed chunks.
/// - Invalidation of cached `SolarChunkFrame` entries when `SolarChunkHead` revision advances.
/// - Cache hit path: avoids patch sorting and vector reconstruction when head revision matches.
/// - Cleanup guarantee: detaching a world drops all cached frames, revisions, and buffers for that world key.
#[derive(Debug)]
pub struct SolarNativeCache {
    config: SolarCacheConfig,
    metrics: SolarCacheMetrics,
    attached_worlds: HashMap<u64, SolarWorldCacheState>,
    access_counter: u64,
}

impl Default for SolarNativeCache {
    fn default() -> Self {
        Self::new(SolarCacheConfig::default())
    }
}

impl SolarNativeCache {
    /// Constructs a new native cache with explicit configuration limits.
    pub fn new(config: SolarCacheConfig) -> Self {
        Self {
            config,
            metrics: SolarCacheMetrics::default(),
            attached_worlds: HashMap::new(),
            access_counter: 0,
        }
    }

    /// Returns a reference to the cache configuration.
    pub fn config(&self) -> &SolarCacheConfig {
        &self.config
    }

    /// Returns a reference to the live cache metrics.
    pub fn metrics(&self) -> &SolarCacheMetrics {
        &self.metrics
    }

    /// Attaches a world to the native cache.
    /// Returns `true` if newly attached, `false` if already present.
    pub fn attach_world(&mut self, world_key: u64) -> bool {
        if let std::collections::hash_map::Entry::Vacant(e) = self.attached_worlds.entry(world_key)
        {
            e.insert(SolarWorldCacheState::new(world_key));
            true
        } else {
            false
        }
    }

    /// Detaches a world and immediately frees all cached chunk frames and revision indices.
    /// Returns `true` if world was attached, `false` otherwise.
    pub fn detach_world(&mut self, world_key: u64) -> bool {
        if let Some(state) = self.attached_worlds.remove(&world_key) {
            self.metrics.current_bytes =
                self.metrics.current_bytes.saturating_sub(state.total_bytes);
            self.metrics.current_frames = self
                .metrics
                .current_frames
                .saturating_sub(state.entries.len());
            for entry in state.entries.values() {
                if entry.is_pinned() {
                    self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                }
            }
            true
        } else {
            false
        }
    }

    /// Returns `true` if the specified world is currently attached.
    pub fn is_world_attached(&self, world_key: u64) -> bool {
        self.attached_worlds.contains_key(&world_key)
    }

    /// Returns the total count of attached worlds.
    pub fn active_world_count(&self) -> usize {
        self.attached_worlds.len()
    }

    /// Returns a list of all currently attached world keys.
    pub fn attached_world_keys(&self) -> Vec<u64> {
        self.attached_worlds.keys().copied().collect()
    }

    /// Returns the number of cached frames for a given world.
    pub fn cached_frame_count(&self, world_key: u64) -> usize {
        self.attached_worlds
            .get(&world_key)
            .map(|s| s.entries.len())
            .unwrap_or(0)
    }

    /// Returns the total bytes allocated to cached frames for a given world.
    pub fn cached_byte_count(&self, world_key: u64) -> usize {
        self.attached_worlds
            .get(&world_key)
            .map(|s| s.total_bytes)
            .unwrap_or(0)
    }

    /// Pins a chunk frame in memory to prevent eviction (e.g. while players are viewing the chunk).
    /// Increments the chunk's pin reference count.
    pub fn pin_chunk(&mut self, chunk_key: u128) -> bool {
        let (world_key, _, _) = unpack_chunk_key(chunk_key);
        if let Some(state) = self.attached_worlds.get_mut(&world_key)
            && let Some(entry) = state.entries.get_mut(&chunk_key)
        {
            if entry.pin_count == 0 {
                self.metrics.pinned_frames += 1;
            }
            entry.pin_count += 1;
            return true;
        }
        false
    }

    /// Decrements the chunk's pin reference count. Once zero, the chunk becomes eligible for LRU eviction.
    pub fn unpin_chunk(&mut self, chunk_key: u128) -> bool {
        let (world_key, _, _) = unpack_chunk_key(chunk_key);
        if let Some(state) = self.attached_worlds.get_mut(&world_key)
            && let Some(entry) = state.entries.get_mut(&chunk_key)
            && entry.pin_count > 0
        {
            entry.pin_count -= 1;
            if entry.pin_count == 0 {
                self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
            }
            return true;
        }
        false
    }

    /// Checks if a chunk is actively pinned.
    pub fn is_chunk_pinned(&self, chunk_key: u128) -> bool {
        let (world_key, _, _) = unpack_chunk_key(chunk_key);
        self.attached_worlds
            .get(&world_key)
            .and_then(|s| s.entries.get(&chunk_key))
            .map(|e| e.is_pinned())
            .unwrap_or(false)
    }

    /// Notifies cache of a chunk head update event from SpacetimeDB.
    /// If the head revision has advanced, any cached frame for this chunk is invalidated.
    pub fn on_chunk_head_updated(&mut self, head: &SolarChunkHead) {
        if let Some(state) = self.attached_worlds.get_mut(&head.world_key) {
            let prev = state
                .head_revisions
                .insert(head.chunk_key, head.applied_revision);
            if let Some(prev_rev) = prev
                && prev_rev != head.applied_revision
                && let Some(removed) = state.remove_entry(&head.chunk_key)
            {
                self.metrics.current_bytes =
                    self.metrics.current_bytes.saturating_sub(removed.byte_size);
                self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                if removed.is_pinned() {
                    self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                }
            }
        }
    }

    /// Retrieves a cached frame or reconstructs it, caching the result and applying LRU eviction if needed.
    pub fn get_or_reconstruct(
        &mut self,
        chunk_key: u128,
        base: Option<&SolarChunkBase>,
        head: Option<&SolarChunkHead>,
        patches: &[SolarChunkPatch],
    ) -> Result<SolarChunkFrame, SolarReadError> {
        self.access_counter += 1;
        let access_seq = self.access_counter;
        let (world_key, _, _) = unpack_chunk_key(chunk_key);

        // 1. Check for fast cache hit
        if let Some(state) = self.attached_worlds.get_mut(&world_key) {
            if let Some(h) = head
                && let Some(entry) = state.entries.get_mut(&chunk_key)
                && entry.frame.applied_revision == h.applied_revision
            {
                entry.last_access_seq = access_seq;
                self.metrics.cache_hits += 1;
                return Ok(entry.frame.clone());
            }
        } else {
            return Err(SolarReadError::NotFound { chunk_key });
        }

        // 2. Cache miss: reconstruct frame
        self.metrics.cache_misses += 1;
        let frame = reconstruct_chunk_frame(chunk_key, base, head, patches)?;
        self.metrics.reconstructions += 1;

        let frame_bytes = frame.approximate_byte_size();

        // 3. Store frame and update metrics
        if let Some(state) = self.attached_worlds.get_mut(&world_key) {
            state
                .head_revisions
                .insert(chunk_key, frame.applied_revision);
            let prev_entry = state.remove_entry(&chunk_key);
            if let Some(p) = prev_entry {
                self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(p.byte_size);
                self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                if p.is_pinned() {
                    self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                }
            }

            state.insert_entry(chunk_key, frame.clone(), access_seq);
            self.metrics.current_bytes += frame_bytes;
            self.metrics.current_frames += 1;
        }

        // 4. Enforce memory and frame count limits (per-world and global)
        self.enforce_eviction_limits(world_key);

        Ok(frame)
    }

    /// Explicitly invalidates a cached chunk frame by packed `chunk_key`.
    pub fn invalidate_chunk(&mut self, chunk_key: u128) -> bool {
        let (world_key, _, _) = unpack_chunk_key(chunk_key);
        if let Some(state) = self.attached_worlds.get_mut(&world_key)
            && let Some(removed) = state.remove_entry(&chunk_key)
        {
            self.metrics.current_bytes =
                self.metrics.current_bytes.saturating_sub(removed.byte_size);
            self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
            if removed.is_pinned() {
                self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
            }
            return true;
        }
        false
    }

    /// Enforces per-world and global memory and frame limits via LRU eviction of unpinned frames.
    fn enforce_eviction_limits(&mut self, target_world_key: u64) {
        // 1. Per-world eviction
        if let Some(state) = self.attached_worlds.get_mut(&target_world_key) {
            while state.total_bytes > self.config.max_world_bytes
                || state.entries.len() > self.config.max_world_frames
            {
                // Find oldest unpinned entry in target world
                let oldest_unpinned = state
                    .entries
                    .iter()
                    .filter(|(_, e)| !e.is_pinned())
                    .min_by_key(|(_, e)| e.last_access_seq)
                    .map(|(k, _)| *k);

                if let Some(evict_key) = oldest_unpinned {
                    if let Some(removed) = state.remove_entry(&evict_key) {
                        self.metrics.evictions_lru += 1;
                        self.metrics.evicted_bytes += removed.byte_size as u64;
                        self.metrics.current_bytes =
                            self.metrics.current_bytes.saturating_sub(removed.byte_size);
                        self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                    }
                } else {
                    // All cached frames are pinned; cannot evict further in this world
                    break;
                }
            }
        }

        // 2. Global eviction across all worlds
        while self.metrics.current_bytes > self.config.max_global_bytes
            || self.metrics.current_frames > self.config.max_global_frames
        {
            // Find global oldest unpinned entry
            let mut global_oldest: Option<(u64, u128, u64)> = None;

            for (w_key, state) in &self.attached_worlds {
                for (c_key, entry) in &state.entries {
                    if !entry.is_pinned() {
                        match global_oldest {
                            None => global_oldest = Some((*w_key, *c_key, entry.last_access_seq)),
                            Some((_, _, oldest_seq)) => {
                                if entry.last_access_seq < oldest_seq {
                                    global_oldest = Some((*w_key, *c_key, entry.last_access_seq));
                                }
                            }
                        }
                    }
                }
            }

            if let Some((w_key, c_key, _)) = global_oldest {
                if let Some(state) = self.attached_worlds.get_mut(&w_key)
                    && let Some(removed) = state.remove_entry(&c_key)
                {
                    self.metrics.evictions_lru += 1;
                    self.metrics.evicted_bytes += removed.byte_size as u64;
                    self.metrics.current_bytes =
                        self.metrics.current_bytes.saturating_sub(removed.byte_size);
                    self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                }
            } else {
                // All globally cached frames are pinned; cannot evict further
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solar_schema::pack_chunk_key;
    use spacetimedb_sdk::Timestamp;

    #[allow(dead_code)]
    fn make_test_frame(world_key: u64, chunk_x: i32, chunk_z: i32, size: usize) -> SolarChunkFrame {
        let chunk_key = pack_chunk_key(world_key, chunk_x, chunk_z);
        SolarChunkFrame {
            chunk_key,
            world_key,
            chunk_x,
            chunk_z,
            applied_revision: 0,
            base_revision: 0,
            base_data: vec![0xAA; size],
            ordered_patches: Vec::new(),
        }
    }

    #[test]
    fn test_byte_bounds_enforced() {
        let config = SolarCacheConfig {
            max_world_bytes: 1000,
            max_world_frames: 100,
            max_global_bytes: 5000,
            max_global_frames: 100,
        };
        let mut cache = SolarNativeCache::new(config);
        assert!(cache.attach_world(1));

        let k1 = pack_chunk_key(1, 0, 0);
        let k2 = pack_chunk_key(1, 1, 0);
        let k3 = pack_chunk_key(1, 2, 0);

        let mk_base = |key, x, sz| SolarChunkBase {
            chunk_key: key,
            world_key: 1,
            chunk_x: x,
            chunk_z: 0,
            data: vec![0xBB; sz],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let mk_head = |key, x| SolarChunkHead {
            chunk_key: key,
            world_key: 1,
            chunk_x: x,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "test".to_string(),
        };

        // Each frame approx size will be around 400+ bytes
        cache.get_or_reconstruct(k1, Some(&mk_base(k1, 0, 400)), Some(&mk_head(k1, 0)), &[]).unwrap();
        cache.get_or_reconstruct(k2, Some(&mk_base(k2, 1, 400)), Some(&mk_head(k2, 1)), &[]).unwrap();

        assert_eq!(cache.cached_frame_count(1), 2);
        assert!(cache.cached_byte_count(1) <= 1000);

        // Inserting k3 exceeds max_world_bytes (400 * 3 > 1000), evicting k1 (LRU)
        cache.get_or_reconstruct(k3, Some(&mk_base(k3, 2, 400)), Some(&mk_head(k3, 2)), &[]).unwrap();

        assert_eq!(cache.cached_frame_count(1), 2);
        assert!(cache.cached_byte_count(1) <= 1000);
        assert_eq!(cache.metrics().evictions_lru, 1);
        assert!(cache.metrics().evicted_bytes > 0);
    }

    #[test]
    fn test_active_authority_data_never_evicted() {
        let config = SolarCacheConfig {
            max_world_bytes: 500,
            max_world_frames: 2,
            max_global_bytes: 1000,
            max_global_frames: 4,
        };
        let mut cache = SolarNativeCache::new(config);
        cache.attach_world(1);

        let k1 = pack_chunk_key(1, 0, 0);
        let k2 = pack_chunk_key(1, 1, 0);
        let k3 = pack_chunk_key(1, 2, 0);

        let mk_base = |key, x| SolarChunkBase {
            chunk_key: key,
            world_key: 1,
            chunk_x: x,
            chunk_z: 0,
            data: vec![0xCC; 10],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let mk_head = |key, x| SolarChunkHead {
            chunk_key: key,
            world_key: 1,
            chunk_x: x,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "test".to_string(),
        };

        cache.get_or_reconstruct(k1, Some(&mk_base(k1, 0)), Some(&mk_head(k1, 0)), &[]).unwrap();
        cache.get_or_reconstruct(k2, Some(&mk_base(k2, 1)), Some(&mk_head(k2, 1)), &[]).unwrap();

        // Pin k1 (oldest)
        assert!(cache.pin_chunk(k1));
        assert!(cache.is_chunk_pinned(k1));

        // Insert k3 -> would evict oldest (k1), but k1 is pinned so k2 is evicted
        cache.get_or_reconstruct(k3, Some(&mk_base(k3, 2)), Some(&mk_head(k3, 2)), &[]).unwrap();

        assert_eq!(cache.cached_frame_count(1), 2);
        assert!(cache.is_chunk_pinned(k1));
    }

    #[test]
    fn test_pin_tokens_release_exactly_once() {
        let mut cache = SolarNativeCache::default();
        cache.attach_world(1);
        let k1 = pack_chunk_key(1, 0, 0);

        let base = SolarChunkBase {
            chunk_key: k1,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0xDD; 10],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head = SolarChunkHead {
            chunk_key: k1,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "test".to_string(),
        };

        cache.get_or_reconstruct(k1, Some(&base), Some(&head), &[]).unwrap();

        // Double pin
        assert!(cache.pin_chunk(k1));
        assert!(cache.pin_chunk(k1));
        assert_eq!(cache.metrics().pinned_frames, 1);
        assert!(cache.is_chunk_pinned(k1));

        // First unpin -> still pinned (refcount = 1)
        assert!(cache.unpin_chunk(k1));
        assert!(cache.is_chunk_pinned(k1));
        assert_eq!(cache.metrics().pinned_frames, 1);

        // Second unpin -> unpinned (refcount = 0)
        assert!(cache.unpin_chunk(k1));
        assert!(!cache.is_chunk_pinned(k1));
        assert_eq!(cache.metrics().pinned_frames, 0);

        // Third unpin -> false (no active pins)
        assert!(!cache.unpin_chunk(k1));
        assert_eq!(cache.metrics().pinned_frames, 0);
    }

    #[test]
    fn test_detach_reclaims_memory() {
        let mut cache = SolarNativeCache::default();
        cache.attach_world(1);
        cache.attach_world(2);

        let k1 = pack_chunk_key(1, 0, 0);
        let k2 = pack_chunk_key(2, 0, 0);

        let base1 = SolarChunkBase {
            chunk_key: k1,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x11; 50],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head1 = SolarChunkHead {
            chunk_key: k1,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "test".to_string(),
        };

        let base2 = SolarChunkBase {
            chunk_key: k2,
            world_key: 2,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x22; 50],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head2 = SolarChunkHead {
            chunk_key: k2,
            world_key: 2,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "test".to_string(),
        };

        cache.get_or_reconstruct(k1, Some(&base1), Some(&head1), &[]).unwrap();
        cache.get_or_reconstruct(k2, Some(&base2), Some(&head2), &[]).unwrap();
        cache.pin_chunk(k1);

        assert_eq!(cache.active_world_count(), 2);
        assert_eq!(cache.metrics().current_frames, 2);
        assert_eq!(cache.metrics().pinned_frames, 1);
        assert!(cache.metrics().current_bytes > 0);

        // Detach world 1
        assert!(cache.detach_world(1));
        assert_eq!(cache.active_world_count(), 1);
        assert_eq!(cache.metrics().current_frames, 1);
        assert_eq!(cache.metrics().pinned_frames, 0);
        assert!(!cache.is_world_attached(1));
        assert!(cache.is_world_attached(2));

        // Detach world 2
        assert!(cache.detach_world(2));
        assert_eq!(cache.active_world_count(), 0);
        assert_eq!(cache.metrics().current_frames, 0);
        assert_eq!(cache.metrics().current_bytes, 0);
    }

    #[test]
    fn test_bounded_growth_under_continuous_insertions() {
        let config = SolarCacheConfig {
            max_world_bytes: 2000,
            max_world_frames: 5,
            max_global_bytes: 4000,
            max_global_frames: 10,
        };
        let mut cache = SolarNativeCache::new(config);
        cache.attach_world(1);

        for i in 0..100 {
            let k = pack_chunk_key(1, i, 0);
            let base = SolarChunkBase {
                chunk_key: k,
                world_key: 1,
                chunk_x: i,
                chunk_z: 0,
                data: vec![0xEE; 100],
                base_revision: 0,
                created_at: Timestamp::from_micros_since_unix_epoch(1),
                updated_at: Timestamp::from_micros_since_unix_epoch(1),
            };
            let head = SolarChunkHead {
                chunk_key: k,
                world_key: 1,
                chunk_x: i,
                chunk_z: 0,
                applied_revision: 0,
                base_revision: 0,
                patch_count: 0,
                updated_at: Timestamp::from_micros_since_unix_epoch(1),
                last_actor: "test".to_string(),
            };
            cache.get_or_reconstruct(k, Some(&base), Some(&head), &[]).unwrap();

            assert!(cache.cached_frame_count(1) <= 5);
            assert!(cache.metrics().current_bytes <= 2000);
        }

        assert_eq!(cache.cached_frame_count(1), 5);
        assert_eq!(cache.metrics().evictions_lru, 95);
    }
}

