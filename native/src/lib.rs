//! Solar native bridge — minimal native-facing read contract and cache interface.
//!
//! # Solar v1 Persistence Architecture
//! - Single-column `u128 chunk_key` primary keys enable direct O(1) in-memory SDK cache lookups.
//! - Reconstructs base palette chunk + ordered patches into an immutable `SolarChunkFrame`.
//! - Pure reconstruction logic avoids allocating JVM objects or performing unneeded codecs.

pub use solar_schema::{
    COMPACTION_THRESHOLD_PATCHES, IMPORT_PHASE_COMPLETED, IMPORT_PHASE_DISCOVERY,
    IMPORT_PHASE_FAILED, IMPORT_PHASE_STREAMING_BASE, IMPORT_PHASE_VALIDATING,
    MAX_EDITS_PER_PATCH, SolarAuthContext, SolarAuthError, SolarBlockEdit, SolarChunkBase,
    SolarChunkHead, SolarChunkPatch, SolarCompactionRequest, SolarWorld, WORLD_STATUS_ACTIVE,
    WORLD_STATUS_FAILED, WORLD_STATUS_FALLBACK, WORLD_STATUS_IMPORTING, pack_chunk_key,
    unpack_chunk_key,
};

use minestom_stdb::{DbConnection, SubscriptionHandle};
use spacetimedb_sdk::DbContext;

/// Error types returned by the Solar native read contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolarReadError {
    /// Chunk was not found in the client cache or database.
    NotFound { chunk_key: u128 },
    /// Base snapshot revision exceeds head applied revision.
    InvalidRevisionSequence {
        chunk_key: u128,
        base_revision: u64,
        applied_revision: u64,
    },
    /// A revision is missing from the ordered patch log (gap detected).
    RevisionGap {
        chunk_key: u128,
        expected_revision: u64,
        found_revision: u64,
    },
    /// Base chunk payload is empty or invalid.
    EmptyBaseData { chunk_key: u128 },
}

impl std::fmt::Display for SolarReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { chunk_key } => {
                write!(f, "Solar chunk not found for chunk_key: 0x{chunk_key:032X}")
            }
            Self::InvalidRevisionSequence {
                chunk_key,
                base_revision,
                applied_revision,
            } => {
                write!(
                    f,
                    "Solar chunk 0x{chunk_key:032X} has invalid revision sequence: base_revision ({base_revision}) > applied_revision ({applied_revision})"
                )
            }
            Self::RevisionGap {
                chunk_key,
                expected_revision,
                found_revision,
            } => {
                write!(
                    f,
                    "Solar chunk 0x{chunk_key:032X} has revision gap: expected patch revision {expected_revision}, got {found_revision}"
                )
            }
            Self::EmptyBaseData { chunk_key } => {
                write!(
                    f,
                    "Solar chunk 0x{chunk_key:032X} has empty base data payload"
                )
            }
        }
    }
}

impl std::error::Error for SolarReadError {}

/// Immutable patch view representing a single revision increment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolarChunkPatchFrame {
    pub revision: u64,
    pub edits: Vec<SolarBlockEdit>,
    pub actor: String,
}

/// Immutable reconstructed chunk frame returned to the native caller.
///
/// Contains the baseline palette-compressed bytes, the sequence of ordered patches
/// to be applied on top of the base, and exact revision metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolarChunkFrame {
    /// 128-bit packed chunk identifier: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`.
    pub chunk_key: u128,
    /// Stable world identifier.
    pub world_key: u64,
    /// Chunk column X coordinate.
    pub chunk_x: i32,
    /// Chunk column Z coordinate.
    pub chunk_z: i32,
    /// Monotonic head revision of this chunk.
    pub applied_revision: u64,
    /// Revision of the immutable baseline byte blob.
    pub base_revision: u64,
    /// Authoritative baseline binary payload (Solar palette container).
    pub base_data: Vec<u8>,
    /// Chronologically ordered list of patches to apply on top of `base_data`.
    pub ordered_patches: Vec<SolarChunkPatchFrame>,
}

impl SolarChunkFrame {
    /// Returns `true` if this chunk frame requires applying uncompacted delta patches.
    #[inline]
    pub fn has_pending_patches(&self) -> bool {
        !self.ordered_patches.is_empty()
    }

    /// Total number of individual block edits across all pending patches.
    #[inline]
    pub fn total_pending_edits(&self) -> usize {
        self.ordered_patches.iter().map(|p| p.edits.len()).sum()
    }

    /// Computes approximate byte footprint in native memory.
    pub fn approximate_byte_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>() + self.base_data.capacity();
        size += self.ordered_patches.capacity() * std::mem::size_of::<SolarChunkPatchFrame>();
        for p in &self.ordered_patches {
            size += p.edits.capacity() * std::mem::size_of::<SolarBlockEdit>();
            size += p.actor.capacity();
        }
        size
    }
}

/// Pure deterministic reconstruction of a `SolarChunkFrame` from raw table entities.
///
/// Validates:
/// 1. Presence of head or base data.
/// 2. Non-empty base data when base snapshot is present.
/// 3. Monotonic revision consistency (`applied_revision >= base_revision`).
/// 4. Contiguous, gap-free patch ordering from `(base_revision + 1)..=applied_revision`.
pub fn reconstruct_chunk_frame(
    chunk_key: u128,
    base: Option<&SolarChunkBase>,
    head: Option<&SolarChunkHead>,
    patches: &[SolarChunkPatch],
) -> Result<SolarChunkFrame, SolarReadError> {
    let (world_key, chunk_x, chunk_z) = unpack_chunk_key(chunk_key);

    match (base, head) {
        (None, _) => Err(SolarReadError::NotFound { chunk_key }),
        (Some(b), None) => {
            if b.data.is_empty() {
                return Err(SolarReadError::EmptyBaseData { chunk_key });
            }
            Ok(SolarChunkFrame {
                chunk_key,
                world_key,
                chunk_x,
                chunk_z,
                applied_revision: b.base_revision,
                base_revision: b.base_revision,
                base_data: b.data.clone(),
                ordered_patches: Vec::new(),
            })
        }
        (Some(b), Some(h)) => {
            if b.data.is_empty() {
                return Err(SolarReadError::EmptyBaseData { chunk_key });
            }
            let base_data = b.data.clone();

            if h.applied_revision < h.base_revision {
                return Err(SolarReadError::InvalidRevisionSequence {
                    chunk_key,
                    base_revision: h.base_revision,
                    applied_revision: h.applied_revision,
                });
            }

            // Filter patches belonging to this chunk with revision > base_revision
            let mut relevant_patches: Vec<&SolarChunkPatch> = patches
                .iter()
                .filter(|p| {
                    p.chunk_key == chunk_key
                        && p.revision > h.base_revision
                        && p.revision <= h.applied_revision
                })
                .collect();

            // Sort chronologically by revision
            relevant_patches.sort_by_key(|p| p.revision);

            // Validate continuity (no revision gaps)
            let mut expected_rev = h.base_revision + 1;
            let mut ordered_patches = Vec::with_capacity(relevant_patches.len());

            #[allow(clippy::explicit_counter_loop)]
            for p in relevant_patches {
                if p.revision != expected_rev {
                    return Err(SolarReadError::RevisionGap {
                        chunk_key,
                        expected_revision: expected_rev,
                        found_revision: p.revision,
                    });
                }
                ordered_patches.push(SolarChunkPatchFrame {
                    revision: p.revision,
                    edits: p.edits.clone(),
                    actor: p.actor.clone(),
                });
                expected_rev += 1;
            }

            if expected_rev - 1 != h.applied_revision {
                return Err(SolarReadError::RevisionGap {
                    chunk_key,
                    expected_revision: expected_rev,
                    found_revision: h.applied_revision,
                });
            }

            Ok(SolarChunkFrame {
                chunk_key,
                world_key,
                chunk_x,
                chunk_z,
                applied_revision: h.applied_revision,
                base_revision: h.base_revision,
                base_data,
                ordered_patches,
            })
        }
    }
}

pub mod cache;
pub mod codec;
pub mod export;
pub mod import;
pub mod interest;
pub mod recovery;

pub use cache::*;
pub use codec::*;
pub use export::*;
pub use import::*;
pub use interest::*;
pub use recovery::*;

/// Subscribes to one world's Solar v1 tables (`solar_world`, `solar_chunk_base`,
/// `solar_chunk_head`, `solar_chunk_patch`, `solar_compaction_request`) by `world_key`.
///
/// Caller owns the returned `SubscriptionHandle` and must call `.unsubscribe()` on world unload.
pub fn subscribe_world(conn: &DbConnection, world_key: u64) -> SubscriptionHandle {
    conn.subscription_builder().subscribe(vec![
        format!("SELECT * FROM solar_world WHERE world_key = {world_key}"),
        format!("SELECT * FROM solar_chunk_base WHERE world_key = {world_key}"),
        format!("SELECT * FROM solar_chunk_head WHERE world_key = {world_key}"),
        format!("SELECT * FROM solar_chunk_patch WHERE world_key = {world_key}"),
        format!("SELECT * FROM solar_compaction_request WHERE world_key = {world_key}"),
    ])
}

// ---------------------------------------------------------------------------
// Native Contract Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use spacetimedb_sdk::Timestamp;

    #[test]
    fn test_reconstruct_chunk_frame_base_only() {
        let chunk_key = pack_chunk_key(10, 2, -4);
        let base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 2,
            chunk_z: -4,
            data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(10),
            updated_at: Timestamp::from_micros_since_unix_epoch(10),
        };
        let head = SolarChunkHead {
            chunk_key,
            world_key: 10,
            chunk_x: 2,
            chunk_z: -4,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(10),
            last_actor: "system".to_string(),
        };

        let frame = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &[]).unwrap();

        assert_eq!(frame.chunk_key, chunk_key);
        assert_eq!(frame.world_key, 10);
        assert_eq!(frame.chunk_x, 2);
        assert_eq!(frame.chunk_z, -4);
        assert_eq!(frame.applied_revision, 0);
        assert_eq!(frame.base_revision, 0);
        assert_eq!(frame.base_data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(!frame.has_pending_patches());
        assert_eq!(frame.total_pending_edits(), 0);
    }

    #[test]
    fn test_reconstruct_chunk_frame_with_ordered_patches() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x01, 0x02],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head = SolarChunkHead {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 3,
            base_revision: 0,
            patch_count: 3,
            updated_at: Timestamp::from_micros_since_unix_epoch(30),
            last_actor: "actor_3".to_string(),
        };

        // Out-of-order patches to verify sorting
        let patches = vec![
            SolarChunkPatch {
                id: 3,
                chunk_key,
                world_key: 10,
                chunk_x: 0,
                chunk_z: 0,
                revision: 3,
                edits: vec![SolarBlockEdit::new(2, 64, 2, 30)],
                actor: "actor_3".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(30),
            },
            SolarChunkPatch {
                id: 1,
                chunk_key,
                world_key: 10,
                chunk_x: 0,
                chunk_z: 0,
                revision: 1,
                edits: vec![
                    SolarBlockEdit::new(0, 64, 0, 10),
                    SolarBlockEdit::new(1, 64, 1, 11),
                ],
                actor: "actor_1".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(10),
            },
            SolarChunkPatch {
                id: 2,
                chunk_key,
                world_key: 10,
                chunk_x: 0,
                chunk_z: 0,
                revision: 2,
                edits: vec![SolarBlockEdit::new(1, 64, 1, 20)],
                actor: "actor_2".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(20),
            },
        ];

        let frame = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &patches).unwrap();

        assert_eq!(frame.applied_revision, 3);
        assert_eq!(frame.base_revision, 0);
        assert!(frame.has_pending_patches());
        assert_eq!(frame.ordered_patches.len(), 3);
        assert_eq!(frame.ordered_patches[0].revision, 1);
        assert_eq!(frame.ordered_patches[1].revision, 2);
        assert_eq!(frame.ordered_patches[2].revision, 3);
        assert_eq!(frame.total_pending_edits(), 4);
    }

    #[test]
    fn test_reconstruct_chunk_frame_not_found() {
        let chunk_key = pack_chunk_key(99, 1, 1);
        let res = reconstruct_chunk_frame(chunk_key, None, None, &[]);
        assert_eq!(res, Err(SolarReadError::NotFound { chunk_key }));
    }

    #[test]
    fn test_reconstruct_chunk_frame_revision_gap() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x01],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head = SolarChunkHead {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 3,
            base_revision: 0,
            patch_count: 2,
            updated_at: Timestamp::from_micros_since_unix_epoch(30),
            last_actor: "actor".to_string(),
        };

        // Missing patch for revision 2
        let patches = vec![
            SolarChunkPatch {
                id: 1,
                chunk_key,
                world_key: 10,
                chunk_x: 0,
                chunk_z: 0,
                revision: 1,
                edits: vec![SolarBlockEdit::new(0, 64, 0, 1)],
                actor: "actor".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(10),
            },
            SolarChunkPatch {
                id: 3,
                chunk_key,
                world_key: 10,
                chunk_x: 0,
                chunk_z: 0,
                revision: 3,
                edits: vec![SolarBlockEdit::new(0, 64, 0, 3)],
                actor: "actor".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(30),
            },
        ];

        let res = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &patches);
        assert_eq!(
            res,
            Err(SolarReadError::RevisionGap {
                chunk_key,
                expected_revision: 2,
                found_revision: 3,
            })
        );
    }

    #[test]
    fn test_reconstruct_chunk_frame_invalid_revision_sequence() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x01],
            base_revision: 5,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head = SolarChunkHead {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 2, // invalid: applied < base
            base_revision: 5,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "actor".to_string(),
        };

        let res = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &[]);
        assert_eq!(
            res,
            Err(SolarReadError::InvalidRevisionSequence {
                chunk_key,
                base_revision: 5,
                applied_revision: 2,
            })
        );
    }

    #[test]
    fn test_reconstruct_chunk_frame_empty_base_data() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![], // empty
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };
        let head = SolarChunkHead {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 0,
            base_revision: 0,
            patch_count: 0,
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
            last_actor: "actor".to_string(),
        };

        let res = reconstruct_chunk_frame(chunk_key, Some(&base), Some(&head), &[]);
        assert_eq!(res, Err(SolarReadError::EmptyBaseData { chunk_key }));
    }

    // -----------------------------------------------------------------------
    // Phase 4 Tests: Bounded Cache, LRU Eviction, Pinning, Tiles, Priority
    // -----------------------------------------------------------------------

    #[test]
    fn test_cache_limits_and_lru_eviction_order() {
        // Cache configured to hold at most 3 frames per world
        let config = SolarCacheConfig {
            max_world_bytes: 1024 * 1024,
            max_world_frames: 3,
            max_global_bytes: 10 * 1024 * 1024,
            max_global_frames: 10,
        };
        let mut cache = SolarNativeCache::new(config);
        cache.attach_world(1);

        let k1 = pack_chunk_key(1, 0, 0);
        let k2 = pack_chunk_key(1, 1, 0);
        let k3 = pack_chunk_key(1, 2, 0);
        let k4 = pack_chunk_key(1, 3, 0);

        let mk_base = |key, x| SolarChunkBase {
            chunk_key: key,
            world_key: 1,
            chunk_x: x,
            chunk_z: 0,
            data: vec![0xAA, 0xBB],
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
            last_actor: "init".to_string(),
        };

        // Insert frames 1, 2, 3
        cache
            .get_or_reconstruct(k1, Some(&mk_base(k1, 0)), Some(&mk_head(k1, 0)), &[])
            .unwrap();
        cache
            .get_or_reconstruct(k2, Some(&mk_base(k2, 1)), Some(&mk_head(k2, 1)), &[])
            .unwrap();
        cache
            .get_or_reconstruct(k3, Some(&mk_base(k3, 2)), Some(&mk_head(k3, 2)), &[])
            .unwrap();

        assert_eq!(cache.cached_frame_count(1), 3);
        assert_eq!(cache.metrics().evictions_lru, 0);

        // Touch k1 to make k2 the oldest
        cache
            .get_or_reconstruct(k1, Some(&mk_base(k1, 0)), Some(&mk_head(k1, 0)), &[])
            .unwrap();

        // Insert k4 -> should evict k2 (oldest LRU entry)
        cache
            .get_or_reconstruct(k4, Some(&mk_base(k4, 3)), Some(&mk_head(k4, 3)), &[])
            .unwrap();

        assert_eq!(cache.cached_frame_count(1), 3);
        assert_eq!(cache.metrics().evictions_lru, 1);
        assert!(cache.metrics().evicted_bytes > 0);
    }

    #[test]
    fn test_cache_pinned_chunks_never_evicted() {
        let config = SolarCacheConfig {
            max_world_bytes: 1024 * 1024,
            max_world_frames: 2,
            max_global_bytes: 10 * 1024 * 1024,
            max_global_frames: 10,
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
            data: vec![0x11],
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
            last_actor: "init".to_string(),
        };

        // Insert k1 and k2
        cache
            .get_or_reconstruct(k1, Some(&mk_base(k1, 0)), Some(&mk_head(k1, 0)), &[])
            .unwrap();
        cache
            .get_or_reconstruct(k2, Some(&mk_base(k2, 1)), Some(&mk_head(k2, 1)), &[])
            .unwrap();

        // Pin k1 (oldest)
        assert!(cache.pin_chunk(k1));
        assert!(cache.is_chunk_pinned(k1));
        assert_eq!(cache.metrics().pinned_frames, 1);

        // Insert k3 -> would normally evict k1, but k1 is pinned, so k2 is evicted
        cache
            .get_or_reconstruct(k3, Some(&mk_base(k3, 2)), Some(&mk_head(k3, 2)), &[])
            .unwrap();

        assert_eq!(cache.cached_frame_count(1), 2);
        assert!(
            cache.is_chunk_pinned(k1),
            "Pinned chunk k1 must survive eviction"
        );

        // Unpin k1
        assert!(cache.unpin_chunk(k1));
        assert!(!cache.is_chunk_pinned(k1));
        assert_eq!(cache.metrics().pinned_frames, 0);
    }

    #[test]
    fn test_tile_reference_counting_and_duplicate_suppression() {
        let mut tracker = SolarTileTracker::new(100);
        assert_eq!(tracker.active_tile_count(), 0);

        // Player 1 subscribes to tile (0, 0) -> newly active
        assert!(tracker.subscribe_tile(0, 0));
        assert_eq!(tracker.subscriber_count(0, 0), 1);
        assert!(tracker.is_tile_active(0, 0));
        assert_eq!(tracker.active_tile_count(), 1);

        // Player 2 subscribes to same tile (0, 0) -> suppressed duplicate, refcount incremented
        assert!(!tracker.subscribe_tile(0, 0));
        assert_eq!(tracker.subscriber_count(0, 0), 2);
        assert_eq!(tracker.active_tile_count(), 1);

        // Player 1 leaves tile (0, 0) -> tile remains active because Player 2 is still present
        assert!(!tracker.unsubscribe_tile(0, 0));
        assert_eq!(tracker.subscriber_count(0, 0), 1);
        assert!(tracker.is_tile_active(0, 0));

        // Player 2 leaves tile (0, 0) -> refcount drops to 0, tile cleaned up
        assert!(tracker.unsubscribe_tile(0, 0));
        assert_eq!(tracker.subscriber_count(0, 0), 0);
        assert!(!tracker.is_tile_active(0, 0));
        assert_eq!(tracker.active_tile_count(), 0);
    }

    #[test]
    fn test_tile_interest_hysteresis_boundary() {
        // Player at chunk (0, 0), view distance = 8 chunks (1 tile radius), hysteresis = 2 chunks
        let (load_tiles, retain_tiles) = compute_tile_interest_with_hysteresis(0, 0, 8, 2);

        assert!(load_tiles.contains(&(0, 0)));
        assert!(retain_tiles.contains(&(0, 0)));

        // Retention set includes the wider hysteresis buffer
        assert!(retain_tiles.len() >= load_tiles.len());

        let bounds = tile_to_chunk_bounds(0, 0);
        assert_eq!(bounds, (0, 7, 0, 7));

        let keys = tile_chunk_keys(1, 0, 0);
        assert_eq!(keys.len(), 64);
    }

    #[test]
    fn test_priority_calculation_nearest_and_predicted() {
        let px = 10;
        let pz = 10;

        // Chunk directly on player position -> distance = 0
        let p_center = calculate_nearest_priority(10, 10, px, pz);
        assert_eq!(p_center, 0.0);

        // Chunk at (13, 14) -> distance = 5.0
        let p_diag = calculate_nearest_priority(13, 14, px, pz);
        assert!((p_diag - 5.0).abs() < 1e-4);

        // Player moving north (+Z) with velocity = (0.0, 5.0) chunks/sec
        // Chunk ahead at (10, 15) vs chunk behind at (10, 5)
        let p_ahead = calculate_movement_predicted_priority(10, 15, px, pz, 0.0, 5.0, 1.0);
        let p_behind = calculate_movement_predicted_priority(10, 5, px, pz, 0.0, 5.0, 1.0);

        assert!(
            p_ahead < p_behind,
            "Chunk in front of moving player must have higher priority (lower score)"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 5 Tests: Codec Envelope, Fold Engine, Frame Ownership
    // -----------------------------------------------------------------------

    #[test]
    fn test_codec_roundtrip_encode_decode() {
        let payload = vec![0x10, 0x20, 0x30, 0x40, 0x50];
        let envelope = SolarChunkEnvelope::new(42, 5, -8, 1, 3, payload.clone()).unwrap();

        let encoded = envelope.encode();
        assert_eq!(encoded.len(), ENVELOPE_HEADER_SIZE + payload.len());

        let decoded = SolarChunkEnvelope::decode(&encoded).unwrap();
        assert_eq!(decoded.world_key, 42);
        assert_eq!(decoded.chunk_x, 5);
        assert_eq!(decoded.chunk_z, -8);
        assert_eq!(decoded.base_revision, 1);
        assert_eq!(decoded.applied_revision, 3);
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_codec_corruption_checksum_rejection() {
        let payload = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let envelope = SolarChunkEnvelope::new(1, 0, 0, 0, 0, payload).unwrap();
        let mut encoded = envelope.encode();

        // Corrupt a byte in the payload
        let last_idx = encoded.len() - 1;
        encoded[last_idx] ^= 0xFF;

        let res = SolarChunkEnvelope::decode(&encoded);
        assert!(matches!(res, Err(SolarCodecError::ChecksumMismatch { .. })));
    }

    #[test]
    fn test_codec_unsupported_version_rejection() {
        let payload = vec![0x01];
        let envelope = SolarChunkEnvelope::new(1, 0, 0, 0, 0, payload).unwrap();
        let mut encoded = envelope.encode();

        // Overwrite version field (bytes 4..6) with version 99
        encoded[4] = 0;
        encoded[5] = 99;

        let res = SolarChunkEnvelope::decode(&encoded);
        assert!(matches!(
            res,
            Err(SolarCodecError::UnsupportedVersion { version: 99 })
        ));
    }

    #[test]
    fn test_codec_size_limit_rejection() {
        let oversized = vec![0u8; MAX_ENVELOPE_PAYLOAD_SIZE + 1];
        let res = SolarChunkEnvelope::new(1, 0, 0, 0, 0, oversized);
        assert!(matches!(res, Err(SolarCodecError::PayloadTooLarge { .. })));
    }

    #[test]
    fn test_codec_truncated_buffer_rejection() {
        let short_bytes = vec![b'S', b'O', b'L', b'R']; // only 4 bytes instead of 48
        let res = SolarChunkEnvelope::decode(&short_bytes);
        assert!(matches!(res, Err(SolarCodecError::TruncatedBuffer { .. })));
    }

    #[test]
    fn test_pure_rust_base_plus_patch_fold() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let frame = SolarChunkFrame {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 2,
            base_revision: 0,
            base_data: vec![0x01, 0x02, 0x03],
            ordered_patches: vec![
                SolarChunkPatchFrame {
                    revision: 1,
                    edits: vec![SolarBlockEdit::new(0, 64, 0, 10)],
                    actor: "player_1".to_string(),
                },
                SolarChunkPatchFrame {
                    revision: 2,
                    edits: vec![SolarBlockEdit::new(1, 64, 1, 20)],
                    actor: "player_2".to_string(),
                },
            ],
        };

        let folded = fold_chunk_frame(&frame).unwrap();
        assert_eq!(folded.world_key, 10);
        assert_eq!(folded.chunk_x, 0);
        assert_eq!(folded.chunk_z, 0);
        assert_eq!(folded.revision, 2);
        assert_eq!(folded.folded_patch_count, 2);
        assert_eq!(folded.folded_edit_count, 2);
        assert!(!folded.data.is_empty());
    }

    #[test]
    fn test_frame_ownership_handle_immutability_and_tokens() {
        let folded = SolarFoldedChunk {
            world_key: 50,
            chunk_x: 2,
            chunk_z: -3,
            revision: 5,
            data: vec![0xAA, 0xBB, 0xCC],
            folded_patch_count: 0,
            folded_edit_count: 0,
        };

        let handle = SolarFrameHandle::publish(1001, folded);
        assert_eq!(handle.release_token(), 1001);
        assert_eq!(handle.revision(), 5);
        assert_eq!(handle.data(), &[0xAA, 0xBB, 0xCC]);
        assert_eq!(handle.strong_count(), 1);

        // Retain another handle
        let handle_clone = handle.retain_handle();
        assert_eq!(handle.strong_count(), 2);
        assert_eq!(handle_clone.release_token(), 1001);

        drop(handle_clone);
        assert_eq!(handle.strong_count(), 1);
    }

    #[test]
    fn test_codec_boundary_coords_and_keys() {
        let payload = vec![0x00];
        let max_world_key = u64::MAX;
        let max_x = i32::MAX;
        let min_z = i32::MIN;
        let envelope =
            SolarChunkEnvelope::new(max_world_key, max_x, min_z, 0, 0, payload.clone()).unwrap();
        let encoded = envelope.encode();
        let decoded = SolarChunkEnvelope::decode(&encoded).unwrap();
        assert_eq!(decoded.world_key, max_world_key);
        assert_eq!(decoded.chunk_x, max_x);
        assert_eq!(decoded.chunk_z, min_z);
    }

    #[test]
    fn test_stable_canonical_fold() {
        let chunk_key = pack_chunk_key(1, 0, 0);
        let frame1 = SolarChunkFrame {
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 2,
            base_revision: 0,
            base_data: vec![0x11, 0x22],
            ordered_patches: vec![SolarChunkPatchFrame {
                revision: 1,
                edits: vec![SolarBlockEdit::new(0, 64, 0, 10)],
                actor: "player".to_string(),
            }],
        };
        let frame2 = frame1.clone();

        let folded1 = fold_chunk_frame(&frame1).unwrap();
        let folded2 = fold_chunk_frame(&frame2).unwrap();

        assert_eq!(folded1.data, folded2.data);
    }

    // -----------------------------------------------------------------------
    // Revision & Crash Recovery Acceptance Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_one_revision_per_accepted_write() {
        let mut manager = SolarRecoveryManager::new();
        let t0 = Timestamp::from_micros_since_unix_epoch(1000);
        let chunk_key = manager
            .initialize_chunk(1, 0, 0, vec![0x10, 0x20], t0)
            .unwrap();

        assert_eq!(manager.head_revision(chunk_key), Some(0));

        // Write 1
        manager
            .stage_mutation(
                101,
                chunk_key,
                0,
                vec![SolarBlockEdit::new(0, 64, 0, 1)],
                "alice".to_string(),
                t0,
            )
            .unwrap();
        let receipt1 = manager.acknowledge_mutation(101, t0).unwrap();
        assert_eq!(receipt1.committed_revision, 1);
        assert_eq!(manager.head_revision(chunk_key), Some(1));

        // Write 2
        manager
            .stage_mutation(
                102,
                chunk_key,
                1,
                vec![SolarBlockEdit::new(1, 64, 1, 2)],
                "bob".to_string(),
                t0,
            )
            .unwrap();
        let receipt2 = manager.acknowledge_mutation(102, t0).unwrap();
        assert_eq!(receipt2.committed_revision, 2);
        assert_eq!(manager.head_revision(chunk_key), Some(2));

        // Write 3
        manager
            .stage_mutation(
                103,
                chunk_key,
                2,
                vec![SolarBlockEdit::new(2, 64, 2, 3)],
                "carol".to_string(),
                t0,
            )
            .unwrap();
        let receipt3 = manager.acknowledge_mutation(103, t0).unwrap();
        assert_eq!(receipt3.committed_revision, 3);
        assert_eq!(manager.head_revision(chunk_key), Some(3));

        // Verify frame reflects exactly 3 revisions
        let frame = manager.read_chunk_frame(chunk_key, None).unwrap();
        assert_eq!(frame.applied_revision, 3);
        assert_eq!(frame.ordered_patches.len(), 3);
        assert_eq!(frame.ordered_patches[0].revision, 1);
        assert_eq!(frame.ordered_patches[1].revision, 2);
        assert_eq!(frame.ordered_patches[2].revision, 3);
    }

    #[test]
    fn test_crash_before_ack_commits_nothing() {
        let mut manager = SolarRecoveryManager::new();
        let t0 = Timestamp::from_micros_since_unix_epoch(1000);
        let chunk_key = manager
            .initialize_chunk(1, 0, 0, vec![0xAA, 0xBB], t0)
            .unwrap();

        // Stage 3 mutations
        manager
            .stage_mutation(
                201,
                chunk_key,
                0,
                vec![SolarBlockEdit::new(0, 64, 0, 1)],
                "alice".to_string(),
                t0,
            )
            .unwrap();

        assert_eq!(manager.pending_staged_count(), 1);
        assert_eq!(manager.head_revision(chunk_key), Some(0));

        // Simulate crash before ACK is received
        let restart_summary = manager.simulate_crash_and_restart();
        assert_eq!(restart_summary.discarded_unacknowledged_mutations, 1);
        assert_eq!(restart_summary.recovered_acknowledged_mutations, 0);

        // Verify chunk remains strictly at revision 0 with zero committed patches
        assert_eq!(manager.head_revision(chunk_key), Some(0));
        assert_eq!(manager.pending_staged_count(), 0);
        assert_eq!(manager.committed_count(), 0);

        let frame = manager.read_chunk_frame(chunk_key, None).unwrap();
        assert_eq!(frame.applied_revision, 0);
        assert_eq!(frame.ordered_patches.len(), 0);

        // Re-attempting ACK on discarded unacknowledged mutation fails
        let ack_res = manager.acknowledge_mutation(201, t0);
        assert!(matches!(
            ack_res,
            Err(SolarRecoveryError::UnacknowledgedMutationDropped { .. })
        ));
    }

    #[test]
    fn test_crash_after_ack_recovers_once() {
        let mut manager = SolarRecoveryManager::new();
        let t0 = Timestamp::from_micros_since_unix_epoch(1000);
        let chunk_key = manager
            .initialize_chunk(1, 0, 0, vec![0xCA, 0xFE], t0)
            .unwrap();

        // Stage & ACK mutation 1
        manager
            .stage_mutation(
                301,
                chunk_key,
                0,
                vec![SolarBlockEdit::new(5, 64, 5, 10)],
                "alice".to_string(),
                t0,
            )
            .unwrap();
        let receipt1 = manager.acknowledge_mutation(301, t0).unwrap();
        assert_eq!(receipt1.committed_revision, 1);

        // Stage mutation 2 (unacknowledged)
        manager
            .stage_mutation(
                302,
                chunk_key,
                1,
                vec![SolarBlockEdit::new(6, 64, 6, 20)],
                "bob".to_string(),
                t0,
            )
            .unwrap();

        // Crash happens: mutation 1 was ACKed, mutation 2 was un-ACKed
        let summary = manager.simulate_crash_and_restart();
        assert_eq!(summary.discarded_unacknowledged_mutations, 1);
        assert_eq!(summary.recovered_acknowledged_mutations, 1);

        // Replay and state verification
        assert_eq!(manager.head_revision(chunk_key), Some(1));
        let frame = manager.read_chunk_frame(chunk_key, None).unwrap();
        assert_eq!(frame.applied_revision, 1);
        assert_eq!(frame.ordered_patches.len(), 1);
        assert_eq!(frame.ordered_patches[0].revision, 1);
        assert_eq!(frame.ordered_patches[0].edits[0].state, 10);

        // Duplicate staging of already acknowledged mutation is rejected
        let dup_res = manager.stage_mutation(
            301,
            chunk_key,
            1,
            vec![SolarBlockEdit::new(5, 64, 5, 10)],
            "alice".to_string(),
            t0,
        );
        assert!(matches!(
            dup_res,
            Err(SolarRecoveryError::DuplicateMutationId { .. })
        ));
    }

    #[test]
    fn test_stale_and_gap_reads_fail_closed() {
        let mut manager = SolarRecoveryManager::new();
        let t0 = Timestamp::from_micros_since_unix_epoch(1000);
        let chunk_key = manager
            .initialize_chunk(1, 0, 0, vec![0x01, 0x02], t0)
            .unwrap();

        // Commit revisions 1, 2, 3
        for rev in 1..=3 {
            manager
                .stage_mutation(
                    400 + rev,
                    chunk_key,
                    rev - 1,
                    vec![SolarBlockEdit::new(0, (60 + rev) as i16, 0, rev as i32)],
                    "tester".to_string(),
                    t0,
                )
                .unwrap();
            manager.acknowledge_mutation(400 + rev, t0).unwrap();
        }

        assert_eq!(manager.head_revision(chunk_key), Some(3));

        // 1. Reading unknown chunk fails closed
        let not_found_key = pack_chunk_key(999, 10, 10);
        assert!(matches!(
            manager.read_chunk_frame(not_found_key, None),
            Err(SolarRecoveryError::NotFound { .. })
        ));

        // 2. Requesting future generation fails closed
        assert!(matches!(
            manager.read_chunk_frame(chunk_key, Some(10)),
            Err(SolarRecoveryError::StaleGeneration { .. })
        ));

        // 3. Inject revision gap (delete revision 2) -> read fails closed
        assert!(manager.inject_revision_gap(chunk_key, 2));
        assert!(matches!(
            manager.read_chunk_frame(chunk_key, None),
            Err(SolarRecoveryError::RevisionGap {
                expected_revision: 2,
                found_revision: 3,
                ..
            })
        ));
    }

    #[test]
    fn test_deterministic_replay_and_checksum_consistency() {
        let edits_1 = vec![SolarBlockEdit::new(0, 64, 0, 1)];
        let edits_2 = vec![SolarBlockEdit::new(1, 64, 1, 2)];
        let batches = vec![edits_1, edits_2];
        let base_bytes = vec![0x12, 0x34, 0x56, 0x78];

        let replay_1 =
            SolarReplayRunner::replay_sequence(1, 0, 0, &base_bytes, &batches).unwrap();
        let replay_2 =
            SolarReplayRunner::replay_sequence(1, 0, 0, &base_bytes, &batches).unwrap();

        assert_eq!(replay_1.revision, 2);
        assert_eq!(replay_1.folded_patch_count, 2);
        assert_eq!(replay_1.data, replay_2.data);

        let checksum1 = crate::codec::calculate_checksum(&replay_1.data);
        let checksum2 = crate::codec::calculate_checksum(&replay_2.data);
        assert_eq!(checksum1, checksum2);
    }
}
