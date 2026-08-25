//! Solar native bridge — minimal native-facing read contract and cache interface.
//!
//! # Solar v1 Persistence Architecture
//! - Single-column `u128 chunk_key` primary keys enable direct O(1) in-memory SDK cache lookups.
//! - Reconstructs base palette chunk + ordered patches into an immutable `SolarChunkFrame`.
//! - Pure reconstruction logic avoids allocating JVM objects or performing unneeded codecs.

pub use solar_schema::{
    pack_chunk_key, unpack_chunk_key, SolarAuthContext, SolarAuthError, SolarBlockEdit,
    SolarChunkBase, SolarChunkHead, SolarChunkPatch, SolarCompactionRequest, SolarWorld,
    COMPACTION_THRESHOLD_PATCHES, MAX_EDITS_PER_PATCH,
};

use minestom_stdb::{
    backend_server_link_native_identity_reducer::backend_server_link_native_identity as BackendServerLinkNativeIdentityExt,
    DbConnection, SubscriptionHandle,
};
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
                write!(f, "Solar chunk 0x{chunk_key:032X} has empty base data payload")
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
        (None, None) => Err(SolarReadError::NotFound { chunk_key }),
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
        (base_opt, Some(h)) => {
            let base_data = match base_opt {
                Some(b) => {
                    if b.data.is_empty() {
                        return Err(SolarReadError::EmptyBaseData { chunk_key });
                    }
                    b.data.clone()
                }
                None => Vec::new(),
            };

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

/// Link this connection's identity as the caller's native-bridge identity
/// (`backend_server_link_native_identity`) — must be called once after connect so the
/// Solar reducers' `assert_backend_server` auth gate accepts calls made over this
/// connection.
pub fn link_native_identity(conn: &DbConnection, server_id: String) -> bool {
    conn.reducers
        .backend_server_link_native_identity(server_id)
        .is_ok()
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
                edits: vec![SolarBlockEdit::new(0, 64, 0, 10), SolarBlockEdit::new(1, 64, 1, 11)],
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
}
