//! Bounded native chunk cache, byte-accurate accounting, LRU eviction, and pinning.

use std::collections::HashMap;
use solar_schema::unpack_chunk_key;
use crate::{reconstruct_chunk_frame, SolarChunkBase, SolarChunkFrame, SolarChunkHead, SolarChunkPatch, SolarReadError};

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
            max_world_bytes: 64 * 1024 * 1024,   // 64 MB
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
    pub fn insert_entry(&mut self, chunk_key: u128, frame: SolarChunkFrame, access_seq: u64) -> usize {
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
        if self.attached_worlds.contains_key(&world_key) {
            false
        } else {
            self.attached_worlds.insert(world_key, SolarWorldCacheState::new(world_key));
            true
        }
    }

    /// Detaches a world and immediately frees all cached chunk frames and revision indices.
    /// Returns `true` if world was attached, `false` otherwise.
    pub fn detach_world(&mut self, world_key: u64) -> bool {
        if let Some(state) = self.attached_worlds.remove(&world_key) {
            self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(state.total_bytes);
            self.metrics.current_frames = self.metrics.current_frames.saturating_sub(state.entries.len());
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
        if let Some(state) = self.attached_worlds.get_mut(&world_key) {
            if let Some(entry) = state.entries.get_mut(&chunk_key) {
                if entry.pin_count == 0 {
                    self.metrics.pinned_frames += 1;
                }
                entry.pin_count += 1;
                return true;
            }
        }
        false
    }

    /// Decrements the chunk's pin reference count. Once zero, the chunk becomes eligible for LRU eviction.
    pub fn unpin_chunk(&mut self, chunk_key: u128) -> bool {
        let (world_key, _, _) = unpack_chunk_key(chunk_key);
        if let Some(state) = self.attached_worlds.get_mut(&world_key) {
            if let Some(entry) = state.entries.get_mut(&chunk_key) {
                if entry.pin_count > 0 {
                    entry.pin_count -= 1;
                    if entry.pin_count == 0 {
                        self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                    }
                    return true;
                }
            }
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
            let prev = state.head_revisions.insert(head.chunk_key, head.applied_revision);
            if let Some(prev_rev) = prev {
                if prev_rev != head.applied_revision {
                    if let Some(removed) = state.remove_entry(&head.chunk_key) {
                        self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(removed.byte_size);
                        self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                        if removed.is_pinned() {
                            self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                        }
                    }
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
            if let Some(h) = head {
                if let Some(entry) = state.entries.get_mut(&chunk_key) {
                    if entry.frame.applied_revision == h.applied_revision {
                        entry.last_access_seq = access_seq;
                        self.metrics.cache_hits += 1;
                        return Ok(entry.frame.clone());
                    }
                }
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
            state.head_revisions.insert(chunk_key, frame.applied_revision);
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
        if let Some(state) = self.attached_worlds.get_mut(&world_key) {
            if let Some(removed) = state.remove_entry(&chunk_key) {
                self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(removed.byte_size);
                self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                if removed.is_pinned() {
                    self.metrics.pinned_frames = self.metrics.pinned_frames.saturating_sub(1);
                }
                return true;
            }
        }
        false
    }

    /// Enforces per-world and global memory and frame limits via LRU eviction of unpinned frames.
    fn enforce_eviction_limits(&mut self, target_world_key: u64) {
        // 1. Per-world eviction
        if let Some(state) = self.attached_worlds.get_mut(&target_world_key) {
            while state.total_bytes > self.config.max_world_bytes || state.entries.len() > self.config.max_world_frames {
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
                        self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(removed.byte_size);
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
                if let Some(state) = self.attached_worlds.get_mut(&w_key) {
                    if let Some(removed) = state.remove_entry(&c_key) {
                        self.metrics.evictions_lru += 1;
                        self.metrics.evicted_bytes += removed.byte_size as u64;
                        self.metrics.current_bytes = self.metrics.current_bytes.saturating_sub(removed.byte_size);
                        self.metrics.current_frames = self.metrics.current_frames.saturating_sub(1);
                    }
                }
            } else {
                // All globally cached frames are pinned; cannot evict further
                break;
            }
        }
    }
}
