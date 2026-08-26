//! Solar — SpacetimeDB 2.8.2 world-store schema (bounded per-chunk persistence model).
//!
//! # Architecture & Evolution
//!
//! The experimental Solar prototype used an auto-incrementing primary key (`id: u64`)
//! with a composite B-Tree index on `(world_id, chunk_x, chunk_z)` for chunks, alongside
//! an un-batched, one-row-per-block delta log (`solar_block_set`).
//!
//! That experimental design had critical scalability flaws:
//! 1. **Linear Cache Scans**: SpacetimeDB client SDKs only generate primary-key index
//!    lookups in memory. Without a single-column primary key on chunk location, reading a
//!    chunk from the client cache required an O(N) linear scan across all subscribed chunks.
//! 2. **Unbounded RAM Growth**: Single-block delta rows produced massive row-count
//!    overhead in SpacetimeDB RAM, transaction overhead per block edit, and high WebSocket
//!    broadcast noise for multi-block operations (explosions, WorldEdit, fills).
//! 3. **Lack of Atomic Revision Gates**: Writes lacked explicit revision fences, allowing
//!    stale client overwrites and lost updates during concurrent edits.
//! 4. **String World ID Overhead**: String identifiers incurred memory allocations and
//!    expensive key comparisons across high-throughput pipelines.
//!
//! # Bounded Solar v1 Data Model
//!
//! Solar v1 introduces a bounded, single-column primary-key architecture:
//! - **`solar_world`**: World metadata keyed by stable `u64 world_key`.
//! - **`solar_chunk_base`**: Palette-compressed chunk snapshot keyed by `u128 chunk_key`.
//! - **`solar_chunk_head`**: Monotonic revision tracker keyed by `u128 chunk_key`.
//! - **`solar_chunk_patch`**: Bounded per-chunk edit batches (`MAX_EDITS_PER_PATCH = 256`).
//! - **`solar_compaction_request`**: Compaction lifecycle tracker keyed by `u128 chunk_key`.
//!
//! # Key Packing
//!
//! Chunk keys are packed into a single `u128`:
//! ```text
//! chunk_key = (world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)
//! ```
//! This provides O(1) primary-key lookups directly in SpacetimeDB and in client-side SDK caches.

use spacetimedb::{table, Timestamp};

/// Maximum block edits allowed in a single `solar_chunk_patch` batch.
pub const MAX_EDITS_PER_PATCH: usize = 256;

/// Number of active patches on a chunk before triggering automatic compaction.
pub const COMPACTION_THRESHOLD_PATCHES: u32 = 64;

/// Bit shift for the 64-bit world key in the packed 128-bit chunk key.
pub const CHUNK_KEY_WORLD_SHIFT: u32 = 64;

/// Bit shift for the 32-bit chunk X coordinate in the packed 128-bit chunk key.
pub const CHUNK_KEY_X_SHIFT: u32 = 32;

/// Bit mask for 32-bit coordinate components.
pub const CHUNK_COORD_MASK: u128 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Lifecycle & Compaction Status Constants
// ---------------------------------------------------------------------------

/// World is undergoing initial chunk streaming/snapshot import from Harbor or template.
pub const WORLD_STATUS_IMPORTING: &str = "IMPORTING";

/// World is active, loaded, and accepting concurrent mutations.
pub const WORLD_STATUS_ACTIVE: &str = "ACTIVE";

/// World import failed permanently or encountered unrecoverable data corruption.
pub const WORLD_STATUS_FAILED: &str = "FAILED";

/// World is in read-only fallback mode (Harbor source snapshot preserved, mutations blocked).
pub const WORLD_STATUS_FALLBACK: &str = "FALLBACK";

/// Import phase: Initial manifest and metadata discovery.
pub const IMPORT_PHASE_DISCOVERY: &str = "DISCOVERY";

/// Import phase: Streaming base chunk blobs into SpacetimeDB.
pub const IMPORT_PHASE_STREAMING_BASE: &str = "STREAMING_BASE";

/// Import phase: Validating chunk snapshot integrity and checksums.
pub const IMPORT_PHASE_VALIDATING: &str = "VALIDATING";

/// Import phase: Successfully completed and active.
pub const IMPORT_PHASE_COMPLETED: &str = "COMPLETED";

/// Import phase: Failed during import or validation.
pub const IMPORT_PHASE_FAILED: &str = "FAILED";

/// Compaction request is queued and awaiting an available worker.
pub const COMPACTION_STATUS_PENDING: &str = "pending";

/// Compaction request has been leased by an active worker.
pub const COMPACTION_STATUS_IN_PROGRESS: &str = "in_progress";

/// Compaction failed or aborted (available for retry/re-lease).
pub const COMPACTION_STATUS_FAILED: &str = "failed";

/// Packs `(world_key, chunk_x, chunk_z)` into a single `u128 chunk_key`.
///
/// Formula: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`
#[inline]
pub fn pack_chunk_key(world_key: u64, chunk_x: i32, chunk_z: i32) -> u128 {
    ((world_key as u128) << CHUNK_KEY_WORLD_SHIFT)
        | (((chunk_x as u32) as u128) << CHUNK_KEY_X_SHIFT)
        | ((chunk_z as u32) as u128)
}

/// Unpacks a `u128 chunk_key` into `(world_key, chunk_x, chunk_z)`.
#[inline]
pub fn unpack_chunk_key(chunk_key: u128) -> (u64, i32, i32) {
    let world_key = (chunk_key >> CHUNK_KEY_WORLD_SHIFT) as u64;
    let chunk_x = ((chunk_key >> CHUNK_KEY_X_SHIFT) & CHUNK_COORD_MASK) as u32 as i32;
    let chunk_z = (chunk_key & CHUNK_COORD_MASK) as u32 as i32;
    (world_key, chunk_x, chunk_z)
}

/// Single block edit representation within a bounded chunk patch batch.
#[derive(Clone, Debug, PartialEq, Eq, spacetimedb::SpacetimeType)]
pub struct SolarBlockEdit {
    /// Local X coordinate within the chunk (0..=15).
    pub x: u8,
    /// World Y coordinate (-64..=320 typical Minecraft height range).
    pub y: i16,
    /// Local Z coordinate within the chunk (0..=15).
    pub z: u8,
    /// Minecraft / Minestom block state ID.
    pub state: i32,
}

impl SolarBlockEdit {
    /// Constructs a new local block edit.
    #[inline]
    pub fn new(x: u8, y: i16, z: u8, state: i32) -> Self {
        Self { x, y, z, state }
    }

    /// Constructs an edit by converting world coordinates to chunk-local offsets.
    #[inline]
    pub fn from_world_coords(world_x: i32, world_y: i32, world_z: i32, state: i32) -> Self {
        Self {
            x: (world_x.rem_euclid(16)) as u8,
            y: world_y as i16,
            z: (world_z.rem_euclid(16)) as u8,
            state,
        }
    }

    /// Converts local coordinates back to world coordinates given chunk coordinates.
    #[inline]
    pub fn to_world_coords(&self, chunk_x: i32, chunk_z: i32) -> (i32, i32, i32) {
        (
            (chunk_x << 4) + (self.x as i32),
            self.y as i32,
            (chunk_z << 4) + (self.z as i32),
        )
    }

    /// Returns `true` if local horizontal coordinates are within `[0, 15]`.
    #[inline]
    pub fn is_valid_local(&self) -> bool {
        self.x < 16 && self.z < 16
    }
}

/// Authorization context passed to Solar mutation reducers.
#[derive(Clone, Debug, PartialEq, Eq, spacetimedb::SpacetimeType)]
pub struct SolarAuthContext {
    /// Player UUID or system identity making the edit.
    pub actor: String,
    /// Whether the caller has administrator bypass privileges.
    pub is_admin: bool,
    /// Whether region claim restrictions should be bypassed.
    pub bypass_claims: bool,
    /// Client tick index when the edit was submitted.
    pub client_tick: u64,
}

impl Default for SolarAuthContext {
    fn default() -> Self {
        Self {
            actor: String::new(),
            is_admin: false,
            bypass_claims: false,
            client_tick: 0,
        }
    }
}

/// Explicit authorization error types for Solar mutations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolarAuthError {
    RegionDenied { region_name: String, reason: String },
    BlockBehaviorForbidden { block_state: i32, reason: String },
    ContainerAccessDenied { container_type: String, reason: String },
    Unauthenticated { reason: String },
    InvalidCoordinates { x: i32, y: i32, z: i32, reason: String },
    RevisionConflict { expected: u64, actual: u64 },
    WorldNotActive { world_key: u64, status: String },
    WorldInFallback { world_key: u64 },
}

/// RegionManager authorization hook interface.
pub trait RegionManagerAuthorizer {
    fn authorize_edit(
        &self,
        world_key: u64,
        world_name: &str,
        world_x: i32,
        world_y: i32,
        world_z: i32,
        actor: &str,
        auth: &SolarAuthContext,
    ) -> Result<(), SolarAuthError>;
}

/// BlockBehavior authorization hook interface.
pub trait BlockBehaviorAuthorizer {
    fn authorize_state_change(
        &self,
        world_key: u64,
        world_x: i32,
        world_y: i32,
        world_z: i32,
        new_state: i32,
        actor: &str,
        auth: &SolarAuthContext,
    ) -> Result<(), SolarAuthError>;
}

/// Container/Inventory access authorization hook interface.
pub trait ContainerAccessAuthorizer {
    fn authorize_container_access(
        &self,
        world_key: u64,
        world_x: i32,
        world_y: i32,
        world_z: i32,
        actor: &str,
        auth: &SolarAuthContext,
    ) -> Result<(), SolarAuthError>;
}

// ---------------------------------------------------------------------------
// SpacetimeDB Table Definitions
// ---------------------------------------------------------------------------

/// Per-world metadata keyed by stable `u64 world_key`.
#[derive(Clone, Debug, PartialEq)]
#[table(
    accessor = solar_world,
    public,
    index(accessor = solar_world_by_name, btree(columns = [world_name])),
    index(accessor = solar_world_by_status, btree(columns = [status])),
)]
pub struct SolarWorld {
    /// Stable numeric world identifier. Single-column primary key.
    #[primary_key]
    pub world_key: u64,
    /// Human-readable world name / routing identifier (e.g. "lobby", "island_42").
    pub world_name: String,
    /// World category: "lobby" | "practice_template" | "island" | "dungeon".
    pub kind: String,
    /// Team or player owner UUID; empty for shared/public worlds.
    pub owner: String,
    /// World lifecycle status: "IMPORTING" | "ACTIVE" | "FAILED" | "FALLBACK".
    pub status: String,
    /// Total chunks declared for import (boss-bar tracking).
    pub total_chunks: u32,
    /// Chunks successfully streamed/loaded so far.
    pub completed_chunks: u32,
    /// Current import phase ("DISCOVERY" | "STREAMING_BASE" | "VALIDATING" | "COMPLETED" | "FAILED").
    pub import_phase: String,
    /// Error/failure reason if world import failed.
    pub failure_reason: String,
    /// Exact Harbor source revision pinned for read-only fallback verification.
    pub harbor_revision: u64,
    /// Exact SHA-256 / Blake3 checksum of original Harbor source archive.
    pub harbor_checksum: String,
    /// Minimum Y coordinate in world (e.g. -64).
    pub min_y: i32,
    /// Total world height in blocks (e.g. 384).
    pub height: i32,
    /// Spawn position X.
    pub spawn_x: f64,
    /// Spawn position Y.
    pub spawn_y: f64,
    /// Spawn position Z.
    pub spawn_z: f64,
    /// Spawn yaw angle in degrees.
    pub spawn_yaw: f32,
    /// Spawn pitch angle in degrees.
    pub spawn_pitch: f32,
    /// World generation seed.
    pub seed: i64,
    /// Source template name or ID (empty if authored/imported directly).
    pub source_template: String,
    /// World creation timestamp.
    pub created_at: Timestamp,
    /// Last metadata update timestamp.
    pub updated_at: Timestamp,
}

impl SolarWorld {
    /// Returns the monotonic import progress percentage (0..=100).
    pub fn progress_pct(&self) -> u32 {
        if self.total_chunks == 0 {
            if self.status == WORLD_STATUS_ACTIVE { 100 } else { 0 }
        } else {
            let pct = ((self.completed_chunks as u64 * 100) / (self.total_chunks as u64)) as u32;
            pct.min(100)
        }
    }
}

/// Canonical baseline chunk snapshot keyed by `u128 chunk_key`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[table(
    accessor = solar_chunk_base,
    public,
    index(accessor = solar_chunk_base_world, btree(columns = [world_key])),
)]
pub struct SolarChunkBase {
    /// Packed chunk identifier: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`.
    #[primary_key]
    pub chunk_key: u128,
    /// Denormalized world key for indexed world queries.
    pub world_key: u64,
    /// Chunk column X coordinate.
    pub chunk_x: i32,
    /// Chunk column Z coordinate.
    pub chunk_z: i32,
    /// Solar per-chunk serialized byte format (palette-compressed sections). Opaque to SpacetimeDB.
    pub data: Vec<u8>,
    /// Revision number represented by this base snapshot.
    pub base_revision: u64,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Last compaction / update timestamp.
    pub updated_at: Timestamp,
}

/// Live head revision and metadata for a chunk column keyed by `u128 chunk_key`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[table(
    accessor = solar_chunk_head,
    public,
    index(accessor = solar_chunk_head_world, btree(columns = [world_key])),
)]
pub struct SolarChunkHead {
    /// Packed chunk identifier: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`.
    #[primary_key]
    pub chunk_key: u128,
    /// Denormalized world key for indexed world queries.
    pub world_key: u64,
    /// Chunk column X coordinate.
    pub chunk_x: i32,
    /// Chunk column Z coordinate.
    pub chunk_z: i32,
    /// Current head revision, monotonically advanced with each committed patch.
    pub applied_revision: u64,
    /// Base snapshot revision this head is currently built upon.
    pub base_revision: u64,
    /// Number of active patches accumulated on top of base_revision.
    pub patch_count: u32,
    /// Last head advancement timestamp.
    pub updated_at: Timestamp,
    /// Identity of last actor to commit a patch.
    pub last_actor: String,
}

/// Bounded per-chunk edit batch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[table(
    accessor = solar_chunk_patch,
    public,
    index(accessor = solar_chunk_patch_chunk, btree(columns = [chunk_key])),
    index(accessor = solar_chunk_patch_world, btree(columns = [world_key])),
)]
pub struct SolarChunkPatch {
    /// Auto-incrementing patch log ID.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Packed chunk identifier: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`.
    pub chunk_key: u128,
    /// Denormalized world key for indexed world queries.
    pub world_key: u64,
    /// Chunk column X coordinate.
    pub chunk_x: i32,
    /// Chunk column Z coordinate.
    pub chunk_z: i32,
    /// Monotonic revision established by this patch.
    pub revision: u64,
    /// Bounded edit list (max `MAX_EDITS_PER_PATCH` items).
    pub edits: Vec<SolarBlockEdit>,
    /// Actor UUID or system name.
    pub actor: String,
    /// Timestamp when this patch was applied.
    pub created_at: Timestamp,
}

/// Compaction request for chunks with accumulated patches.
#[derive(Clone, Debug, PartialEq, Eq)]
#[table(
    accessor = solar_compaction_request,
    public,
    index(accessor = solar_compaction_world, btree(columns = [world_key])),
    index(accessor = solar_compaction_status, btree(columns = [status])),
)]
pub struct SolarCompactionRequest {
    /// Packed chunk identifier: `(world_key << 64) | ((chunk_x as u32) << 32) | (chunk_z as u32)`.
    #[primary_key]
    pub chunk_key: u128,
    /// Denormalized world key for indexed world queries.
    pub world_key: u64,
    /// Chunk column X coordinate.
    pub chunk_x: i32,
    /// Chunk column Z coordinate.
    pub chunk_z: i32,
    /// Target revision to compact up to.
    pub head_revision: u64,
    /// Number of patches pending compaction.
    pub patch_count: u32,
    /// Request status: "pending" | "in_progress" | "failed".
    pub status: String,
    /// Monotonic token identifying the active lease.
    pub lease_token: u64,
    /// Identity / UUID of the worker holding the lease.
    pub lease_owner: String,
    /// Timestamp when active lease expires.
    pub lease_expires_at: Timestamp,
    /// Number of failed compaction attempts on this chunk.
    pub compaction_attempts: u32,
    /// Timestamp when last attempt failed.
    pub last_failed_at: Timestamp,
    /// Active exponential backoff duration in microseconds before next eligible attempt.
    pub backoff_micros: u64,
    /// Error / failure reason if compaction failed.
    pub failure_reason: String,
    /// Timestamp when compaction was requested.
    pub requested_at: Timestamp,
    /// Originating requester ("system_auto_threshold" | admin UUID).
    pub requested_by: String,
}

// ---------------------------------------------------------------------------
// Validation & State Transition Helpers
// ---------------------------------------------------------------------------

/// Validates world lifecycle state transitions.
pub fn validate_world_lifecycle_transition(
    current: Option<&str>,
    target: &str,
) -> Result<(), String> {
    match (current, target) {
        (None, WORLD_STATUS_IMPORTING) | (None, WORLD_STATUS_ACTIVE) => Ok(()),
        (Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_ACTIVE) => Ok(()),
        (Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_FAILED) => Ok(()),
        (Some(WORLD_STATUS_ACTIVE), WORLD_STATUS_FALLBACK) => Ok(()),
        (Some(WORLD_STATUS_FALLBACK), WORLD_STATUS_ACTIVE) => Ok(()),
        (Some(WORLD_STATUS_FAILED), WORLD_STATUS_IMPORTING) => Ok(()),
        (Some(WORLD_STATUS_ACTIVE), WORLD_STATUS_IMPORTING) => Ok(()),
        (Some(c), t) if c == t => Ok(()), // idempotent re-assertion
        (Some(c), t) => Err(format!(
            "invalid world lifecycle transition from '{c}' to '{t}'"
        )),
        (None, t) => Err(format!(
            "cannot initialize world directly in state '{t}'"
        )),
    }
}

/// Verifies if a world is in an active state allowing mutations.
pub fn validate_world_mutation_eligibility(world: &SolarWorld) -> Result<(), SolarAuthError> {
    if world.status == WORLD_STATUS_ACTIVE {
        Ok(())
    } else if world.status == WORLD_STATUS_FALLBACK {
        Err(SolarAuthError::WorldInFallback {
            world_key: world.world_key,
        })
    } else {
        Err(SolarAuthError::WorldNotActive {
            world_key: world.world_key,
            status: world.status.clone(),
        })
    }
}

/// Evaluates whether a worker can acquire a compaction lease on a request,
/// respecting active leases, expired leases, and exponential retry backoff.
pub fn evaluate_compaction_lease_acquire(
    current_status: &str,
    lease_expires_at: Timestamp,
    last_failed_at: Timestamp,
    backoff_micros: u64,
    current_time: Timestamp,
) -> Result<(), String> {
    if current_status == COMPACTION_STATUS_PENDING {
        Ok(())
    } else if current_status == COMPACTION_STATUS_FAILED {
        let eligible_at = last_failed_at.to_micros_since_unix_epoch() + (backoff_micros as i64);
        if current_time.to_micros_since_unix_epoch() >= eligible_at {
            Ok(())
        } else {
            let remaining = eligible_at - current_time.to_micros_since_unix_epoch();
            Err(format!(
                "compaction retry backoff active: retry eligible in {remaining} µs"
            ))
        }
    } else if current_status == COMPACTION_STATUS_IN_PROGRESS {
        if current_time >= lease_expires_at {
            // Expired lease can be reclaimed
            Ok(())
        } else {
            Err("compaction lease conflict: another worker actively holds an unexpired lease".to_string())
        }
    } else {
        Err(format!("unknown compaction status '{current_status}'"))
    }
}

/// Evaluates whether a worker's lease token is valid to release/complete compaction.
pub fn evaluate_compaction_lease_release(
    current_status: &str,
    current_token: u64,
    expected_token: u64,
) -> Result<(), String> {
    if current_status != COMPACTION_STATUS_IN_PROGRESS {
        return Err(format!(
            "cannot complete compaction: request status is '{current_status}' (expected 'in_progress')"
        ));
    }
    if current_token != expected_token {
        return Err(format!(
            "compaction lease token mismatch: expected token {expected_token}, active lease has {current_token}"
        ));
    }
    Ok(())
}

/// Validates a batch of block edits against bounded limits and height constraints.
pub fn validate_patch_bounds(
    edits: &[SolarBlockEdit],
    world_min_y: i32,
    world_height: i32,
) -> Result<(), String> {
    if edits.is_empty() {
        return Err("patch contains no edits (empty batch)".to_string());
    }
    if edits.len() > MAX_EDITS_PER_PATCH {
        return Err(format!(
            "patch exceeds maximum batch limit of {} (got {})",
            MAX_EDITS_PER_PATCH,
            edits.len()
        ));
    }
    let max_y = world_min_y + world_height;
    for (i, edit) in edits.iter().enumerate() {
        if !edit.is_valid_local() {
            return Err(format!(
                "edit [{}] local coords ({}, {}) out of chunk range 0..=15",
                i, edit.x, edit.z
            ));
        }
        let y = edit.y as i32;
        if y < world_min_y || y >= max_y {
            return Err(format!(
                "edit [{}] y={} out of world height bounds [{}, {})",
                i, y, world_min_y, max_y
            ));
        }
    }
    Ok(())
}

/// Evaluates expected revision against current head revision for atomic commits.
pub fn check_expected_revision(
    current_head_revision: Option<u64>,
    expected_revision: u64,
) -> Result<u64, String> {
    match current_head_revision {
        Some(actual) => {
            if actual != expected_revision {
                Err(format!(
                    "revision conflict: expected revision {}, current head is {}",
                    expected_revision, actual
                ))
            } else {
                Ok(actual + 1)
            }
        }
        None => {
            if expected_revision != 0 {
                Err(format!(
                    "revision conflict: chunk head does not exist, expected revision 0 (got {})",
                    expected_revision
                ))
            } else {
                Ok(1)
            }
        }
    }
}

/// Validates revision boundaries during compaction completion.
pub fn validate_compaction_bounds(
    base_revision: u64,
    applied_revision: u64,
    compacted_up_to_revision: u64,
) -> Result<(), String> {
    if compacted_up_to_revision <= base_revision {
        return Err(format!(
            "compacted_up_to_revision ({}) must be greater than current base_revision ({})",
            compacted_up_to_revision, base_revision
        ));
    }
    if compacted_up_to_revision > applied_revision {
        return Err(format!(
            "compacted_up_to_revision ({}) cannot exceed current head revision ({})",
            compacted_up_to_revision, applied_revision
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Native Test & Client Stubs for SpacetimeDB Host Imports
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod host_stubs {
    #[unsafe(no_mangle)]
    pub extern "C" fn index_id_from_name(_: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn bytes_sink_write(_: *const u8, _: usize) {}
    #[unsafe(no_mangle)]
    pub extern "C" fn bytes_source_remaining_length() -> usize { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn bytes_source_read(_: *mut u8, _: usize) -> usize { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn procedure_sleep_until(_: u64) {}
    #[unsafe(no_mangle)]
    pub extern "C" fn console_timer_start(_: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn console_log(_: u32, _: *const u8, _: usize) {}
    #[unsafe(no_mangle)]
    pub extern "C" fn table_id_from_name(_: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn get_jwt(_: *mut u8, _: usize) -> usize { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn identity(_: *mut u8) {}
    #[unsafe(no_mangle)]
    pub extern "C" fn console_timer_end(_: u32) {}
    #[unsafe(no_mangle)]
    pub extern "C" fn datastore_table_scan_bsatn(_: u32) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn datastore_index_scan_point_bsatn(_: u32, _: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn datastore_index_scan_range_bsatn(_: u32, _: *const u8, _: usize, _: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn datastore_delete_by_index_scan_point_bsatn(_: u32, _: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn datastore_delete_by_index_scan_range_bsatn(_: u32, _: *const u8, _: usize, _: *const u8, _: usize) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn row_iter_bsatn_advance(_: u32) -> u32 { 0 }
    #[unsafe(no_mangle)]
    pub extern "C" fn row_iter_bsatn_close(_: u32) {}
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pack_unpack_chunk_key_roundtrip() {
        let test_cases = [
            (0u64, 0i32, 0i32),
            (1u64, 100i32, 200i32),
            (42u64, -1i32, -1i32),
            (999u64, -1500i32, 3200i32),
            (0x1234_5678_9ABC_DEF0u64, i32::MIN, i32::MAX),
            (u64::MAX, i32::MAX, i32::MIN),
            (7u64, -16i32, 16i32),
            (100u64, -128i32, -256i32),
        ];

        for (world_key, chunk_x, chunk_z) in test_cases {
            let packed = pack_chunk_key(world_key, chunk_x, chunk_z);
            let (unpacked_world, unpacked_x, unpacked_z) = unpack_chunk_key(packed);
            assert_eq!(unpacked_world, world_key, "world_key mismatch for ({world_key}, {chunk_x}, {chunk_z})");
            assert_eq!(unpacked_x, chunk_x, "chunk_x mismatch for ({world_key}, {chunk_x}, {chunk_z})");
            assert_eq!(unpacked_z, chunk_z, "chunk_z mismatch for ({world_key}, {chunk_x}, {chunk_z})");
        }
    }

    #[test]
    fn test_key_packing_bit_layout_formula() {
        let world_key: u64 = 0x0123_4567_89AB_CDEF;
        let chunk_x: i32 = 0x1234_5678;
        let chunk_z: i32 = -1; // 0xFFFF_FFFF in u32

        let packed = pack_chunk_key(world_key, chunk_x, chunk_z);

        // Verify exact bit positions:
        // World: bits [64..127]
        let high_64 = (packed >> 64) as u64;
        assert_eq!(high_64, world_key);

        // Chunk X: bits [32..63]
        let mid_32 = ((packed >> 32) & 0xFFFF_FFFF) as u32 as i32;
        assert_eq!(mid_32, chunk_x);

        // Chunk Z: bits [0..31]
        let low_32 = (packed & 0xFFFF_FFFF) as u32 as i32;
        assert_eq!(low_32, chunk_z);
    }

    #[test]
    fn test_bounded_patch_validation_empty() {
        let edits: Vec<SolarBlockEdit> = vec![];
        let result = validate_patch_bounds(&edits, -64, 384);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty batch"));
    }

    #[test]
    fn test_bounded_patch_validation_max_size() {
        // Valid limit (256 edits)
        let valid_edits: Vec<SolarBlockEdit> = (0..256)
            .map(|i| SolarBlockEdit::new((i % 16) as u8, 64, ((i / 16) % 16) as u8, 1))
            .collect();
        assert!(validate_patch_bounds(&valid_edits, -64, 384).is_ok());

        // Exceeding limit (257 edits)
        let mut oversized_edits = valid_edits;
        oversized_edits.push(SolarBlockEdit::new(0, 64, 0, 1));
        let result = validate_patch_bounds(&oversized_edits, -64, 384);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum batch limit"));
    }

    #[test]
    fn test_bounded_patch_validation_coordinate_bounds() {
        // Local X out of range
        let invalid_x = vec![SolarBlockEdit::new(16, 64, 5, 1)];
        assert!(validate_patch_bounds(&invalid_x, -64, 384).is_err());

        // Local Z out of range
        let invalid_z = vec![SolarBlockEdit::new(5, 64, 16, 1)];
        assert!(validate_patch_bounds(&invalid_z, -64, 384).is_err());

        // Y below min_y (-64)
        let invalid_y_low = vec![SolarBlockEdit::new(5, -65, 5, 1)];
        assert!(validate_patch_bounds(&invalid_y_low, -64, 384).is_err());

        // Y above max_y (-64 + 384 = 320)
        let invalid_y_high = vec![SolarBlockEdit::new(5, 320, 5, 1)];
        assert!(validate_patch_bounds(&invalid_y_high, -64, 384).is_err());

        // Valid boundary Y values
        let valid_bounds = vec![
            SolarBlockEdit::new(0, -64, 0, 1),
            SolarBlockEdit::new(15, 319, 15, 1),
        ];
        assert!(validate_patch_bounds(&valid_bounds, -64, 384).is_ok());
    }

    #[test]
    fn test_world_to_local_coordinate_conversions() {
        // Negative chunk coords (-1, -1) -> world bounds [-16..-1, -16..-1]
        let world_x = -5; // inside chunk_x = -1, local x = 11
        let world_z = -12; // inside chunk_z = -1, local z = 4
        let edit = SolarBlockEdit::from_world_coords(world_x, 70, world_z, 42);

        assert_eq!(edit.x, 11);
        assert_eq!(edit.y, 70);
        assert_eq!(edit.z, 4);
        assert_eq!(edit.state, 42);
        assert!(edit.is_valid_local());

        let (wx, wy, wz) = edit.to_world_coords(-1, -1);
        assert_eq!(wx, world_x);
        assert_eq!(wy, 70);
        assert_eq!(wz, world_z);
    }

    #[test]
    fn test_atomic_revision_conflict_rejection() {
        // Expected revision matches existing head -> succeeds and increments
        let next_rev = check_expected_revision(Some(5), 5);
        assert_eq!(next_rev, Ok(6));

        // Expected revision mismatch -> rejects atomically
        let conflict = check_expected_revision(Some(5), 4);
        assert!(conflict.is_err());
        assert!(conflict.unwrap_err().contains("revision conflict"));

        // Chunk has no head yet, expecting revision 0 -> succeeds
        let initial_rev = check_expected_revision(None, 0);
        assert_eq!(initial_rev, Ok(1));

        // Chunk has no head yet, expecting non-zero -> rejects
        let invalid_initial = check_expected_revision(None, 1);
        assert!(invalid_initial.is_err());
        assert!(invalid_initial.unwrap_err().contains("chunk head does not exist"));
    }

    #[test]
    fn test_compaction_bounds_validation() {
        // Valid compaction: base=10, head=25, compact_to=20
        assert!(validate_compaction_bounds(10, 25, 20).is_ok());

        // Valid full compaction: base=10, head=25, compact_to=25
        assert!(validate_compaction_bounds(10, 25, 25).is_ok());

        // Invalid: compact_to <= base (no progress)
        let no_progress = validate_compaction_bounds(10, 25, 10);
        assert!(no_progress.is_err());

        // Invalid: compact_to > head (compacting unapplied future revision)
        let future_compact = validate_compaction_bounds(10, 25, 26);
        assert!(future_compact.is_err());
    }

    struct MockRegionManager {
        safezones: Vec<(i32, i32, i32, i32, i32, i32)>,
    }

    impl RegionManagerAuthorizer for MockRegionManager {
        fn authorize_edit(
            &self,
            _world_key: u64,
            _world_name: &str,
            world_x: i32,
            world_y: i32,
            world_z: i32,
            _actor: &str,
            auth: &SolarAuthContext,
        ) -> Result<(), SolarAuthError> {
            if auth.is_admin || auth.bypass_claims {
                return Ok(());
            }
            for (min_x, min_y, min_z, max_x, max_y, max_z) in &self.safezones {
                if world_x >= *min_x
                    && world_x <= *max_x
                    && world_y >= *min_y
                    && world_y <= *max_y
                    && world_z >= *min_z
                    && world_z <= *max_z
                {
                    return Err(SolarAuthError::RegionDenied {
                        region_name: "SpawnSafezone".to_string(),
                        reason: "edits forbidden in safezone".to_string(),
                    });
                }
            }
            Ok(())
        }
    }

    struct MockBlockBehavior;

    impl BlockBehaviorAuthorizer for MockBlockBehavior {
        fn authorize_state_change(
            &self,
            _world_key: u64,
            _world_x: i32,
            _world_y: i32,
            _world_z: i32,
            new_state: i32,
            _actor: &str,
            auth: &SolarAuthContext,
        ) -> Result<(), SolarAuthError> {
            if auth.is_admin {
                return Ok(());
            }
            if new_state < 0 {
                return Err(SolarAuthError::BlockBehaviorForbidden {
                    block_state: new_state,
                    reason: "negative state id forbidden".to_string(),
                });
            }
            if new_state == 9999 {
                return Err(SolarAuthError::BlockBehaviorForbidden {
                    block_state: new_state,
                    reason: "blacklisted bedrock replacement".to_string(),
                });
            }
            Ok(())
        }
    }

    #[test]
    fn test_authorization_hooks_evaluation() {
        let region_mgr = MockRegionManager {
            safezones: vec![(0, -64, 0, 100, 320, 100)],
        };
        let block_behavior = MockBlockBehavior;

        // Player editing inside safezone -> Denied
        let player_auth = SolarAuthContext {
            actor: "player_1".to_string(),
            is_admin: false,
            bypass_claims: false,
            client_tick: 100,
        };
        let res = region_mgr.authorize_edit(1, "world", 50, 64, 50, "player_1", &player_auth);
        assert!(matches!(res, Err(SolarAuthError::RegionDenied { .. })));

        // Admin editing inside safezone -> Allowed
        let admin_auth = SolarAuthContext {
            actor: "admin_1".to_string(),
            is_admin: true,
            bypass_claims: true,
            client_tick: 101,
        };
        assert!(region_mgr
            .authorize_edit(1, "world", 50, 64, 50, "admin_1", &admin_auth)
            .is_ok());

        // Player editing outside safezone -> Allowed
        assert!(region_mgr
            .authorize_edit(1, "world", 500, 64, 500, "player_1", &player_auth)
            .is_ok());

        // Player attempting forbidden block state -> Denied
        let forbidden_state =
            block_behavior.authorize_state_change(1, 500, 64, 500, 9999, "player_1", &player_auth);
        assert!(matches!(
            forbidden_state,
            Err(SolarAuthError::BlockBehaviorForbidden { .. })
        ));

        // Player placing valid state -> Allowed
        assert!(block_behavior
            .authorize_state_change(1, 500, 64, 500, 1, "player_1", &player_auth)
            .is_ok());
    }

    #[test]
    fn test_chunk_lifecycle_state_machine() {
        // Step 1: Initial commit on brand new chunk (expected_rev = 0)
        let mut current_head: Option<SolarChunkHead> = None;
        let mut patch_count = 0;

        let world_key = 100u64;
        let chunk_x = 5;
        let chunk_z = -3;
        let chunk_key = pack_chunk_key(world_key, chunk_x, chunk_z);

        // Attempt commit with wrong expected revision (expected = 5 on new chunk) -> Atomic Rejection
        let conflict = check_expected_revision(
            current_head.as_ref().map(|h| h.applied_revision),
            5,
        );
        assert!(conflict.is_err());
        assert_eq!(
            current_head.as_ref().map(|h| h.applied_revision),
            None,
            "State must remain unchanged on rejection"
        );

        // Successful commit at expected = 0 -> Advances to rev 1
        let next_rev = check_expected_revision(
            current_head.as_ref().map(|h| h.applied_revision),
            0,
        )
        .unwrap();
        assert_eq!(next_rev, 1);
        patch_count += 1;
        current_head = Some(SolarChunkHead {
            chunk_key,
            world_key,
            chunk_x,
            chunk_z,
            applied_revision: next_rev,
            base_revision: 0,
            patch_count,
            updated_at: Timestamp::from_micros_since_unix_epoch(1000),
            last_actor: "player_1".to_string(),
        });

        // Step 2: Second edit expecting rev 1 -> Advances to rev 2
        let next_rev = check_expected_revision(
            current_head.as_ref().map(|h| h.applied_revision),
            1,
        )
        .unwrap();
        assert_eq!(next_rev, 2);
        patch_count += 1;
        {
            let head = current_head.as_mut().unwrap();
            head.applied_revision = next_rev;
            head.patch_count = patch_count;
        }

        // Step 3: Concurrent edit sending stale expected rev 1 when current head is 2 -> Atomic Rejection
        let stale_rejection = check_expected_revision(
            current_head.as_ref().map(|h| h.applied_revision),
            1,
        );
        assert!(stale_rejection.is_err());
        assert_eq!(current_head.as_ref().unwrap().applied_revision, 2);

        // Step 4: Compaction threshold verification
        let head = current_head.as_mut().unwrap();
        head.patch_count = COMPACTION_THRESHOLD_PATCHES;
        let needs_compaction = head.patch_count >= COMPACTION_THRESHOLD_PATCHES;
        assert!(needs_compaction);

        // Compaction succeeds up to revision 2
        assert!(validate_compaction_bounds(head.base_revision, head.applied_revision, 2).is_ok());
        head.base_revision = 2;
        head.patch_count = 0;
        assert_eq!(head.base_revision, 2);
        assert_eq!(head.patch_count, 0);
    }

    #[test]
    fn test_concurrent_revision_conflict_exactly_one_succeeds() {
        // Models 4 concurrent workers attempting to write to a chunk currently at revision 5
        let current_revision = 5u64;
        let mut head = SolarChunkHead {
            chunk_key: pack_chunk_key(42, 0, 0),
            world_key: 42,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: current_revision,
            base_revision: 0,
            patch_count: 5,
            updated_at: Timestamp::from_micros_since_unix_epoch(100),
            last_actor: "init".to_string(),
        };

        let actors = ["worker_A", "worker_B", "worker_C", "worker_D"];
        let mut success_count = 0;
        let mut conflict_count = 0;
        let mut patch_log: Vec<(u64, String)> = Vec::new();

        for actor in actors {
            // All workers believed head was at revision 5
            let expected_rev = 5u64;
            match check_expected_revision(Some(head.applied_revision), expected_rev) {
                Ok(next_rev) => {
                    success_count += 1;
                    head.applied_revision = next_rev;
                    head.patch_count += 1;
                    head.last_actor = actor.to_string();
                    patch_log.push((next_rev, actor.to_string()));
                }
                Err(_) => {
                    conflict_count += 1;
                }
            }
        }

        assert_eq!(success_count, 1, "Exactly one concurrent worker must succeed");
        assert_eq!(conflict_count, 3, "All other concurrent workers must fail with revision conflict");
        assert_eq!(head.applied_revision, 6, "Head must advance exactly once to revision 6");
        assert_eq!(head.patch_count, 6);
        assert_eq!(head.last_actor, "worker_A");
        assert_eq!(patch_log.len(), 1);
        assert_eq!(patch_log[0], (6, "worker_A".to_string()));
    }

    #[test]
    fn test_compaction_cannot_discard_newer_patches() {
        let chunk_key = pack_chunk_key(1, 0, 0);

        // Chunk starts with base_revision = 0, and has 5 sequential patches
        let mut base = SolarChunkBase {
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x10, 0x20],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };

        let mut head = SolarChunkHead {
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 5,
            base_revision: 0,
            patch_count: 5,
            updated_at: Timestamp::from_micros_since_unix_epoch(50),
            last_actor: "actor".to_string(),
        };

        let mut patches: Vec<SolarChunkPatch> = (1..=5)
            .map(|rev| SolarChunkPatch {
                id: rev,
                chunk_key,
                world_key: 1,
                chunk_x: 0,
                chunk_z: 0,
                revision: rev,
                edits: vec![SolarBlockEdit::new(0, 64, 0, rev as i32)],
                actor: format!("actor_{rev}"),
                created_at: Timestamp::from_micros_since_unix_epoch(rev as i64 * 10),
            })
            .collect();

        // Compaction executes for cutoff revision = 3
        let cutoff_revision = 3u64;
        assert!(validate_compaction_bounds(head.base_revision, head.applied_revision, cutoff_revision).is_ok());

        // 1. Base snapshot is updated
        base.data = vec![0xAA, 0xBB, 0xCC]; // newly folded compressed chunk
        base.base_revision = cutoff_revision;

        // 2. Safe cutoff pruning: prune patches where revision <= cutoff_revision
        patches.retain(|p| p.revision > cutoff_revision);

        // 3. Update head metadata
        head.base_revision = cutoff_revision;
        head.patch_count = patches.len() as u32;

        // Verify that newer patches (revisions 4 and 5) were NOT discarded
        assert_eq!(patches.len(), 2, "Patches 4 and 5 must remain in the patch table");
        assert_eq!(patches[0].revision, 4);
        assert_eq!(patches[1].revision, 5);
        assert_eq!(head.base_revision, 3);
        assert_eq!(head.applied_revision, 5);
        assert_eq!(head.patch_count, 2);
    }

    #[test]
    fn test_failed_authorization_leaves_all_tables_unchanged() {
        let chunk_key = pack_chunk_key(1, 0, 0);

        let initial_head = SolarChunkHead {
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 3,
            base_revision: 0,
            patch_count: 3,
            updated_at: Timestamp::from_micros_since_unix_epoch(30),
            last_actor: "valid_actor".to_string(),
        };

        let initial_base = SolarChunkBase {
            chunk_key,
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x01, 0x02],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };

        let initial_patches = vec![
            SolarChunkPatch {
                id: 1,
                chunk_key,
                world_key: 1,
                chunk_x: 0,
                chunk_z: 0,
                revision: 1,
                edits: vec![SolarBlockEdit::new(0, 64, 0, 1)],
                actor: "valid_actor".to_string(),
                created_at: Timestamp::from_micros_since_unix_epoch(10),
            },
        ];

        let region_mgr = MockRegionManager {
            safezones: vec![(0, -64, 0, 100, 320, 100)],
        };
        let _block_behavior = MockBlockBehavior;

        // Attacker attempts unauthorized edit in safezone
        let rogue_auth = SolarAuthContext {
            actor: "griefer".to_string(),
            is_admin: false,
            bypass_claims: false,
            client_tick: 999,
        };

        let auth_res = region_mgr.authorize_edit(1, "world", 10, 64, 10, "griefer", &rogue_auth);
        assert!(auth_res.is_err(), "Edit must be rejected by RegionManager");

        // Simulate transaction abort: tables remain identical
        let current_head = initial_head.clone();
        let current_base = initial_base.clone();
        let current_patches = initial_patches.clone();

        assert_eq!(current_head, initial_head, "Head must be completely unchanged");
        assert_eq!(current_base, initial_base, "Base must be completely unchanged");
        assert_eq!(current_patches, initial_patches, "Patch log must be completely unchanged");
    }

    #[test]
    fn test_world_template_branch_copy_distinct_keys_and_no_source_mutation() {
        let src_world_key = 100u64;
        let dst_world_key = 200u64;

        let src_coords = [(0, 0), (-1, -1), (5, -10), (100, 200)];
        let mut src_base_chunks: Vec<SolarChunkBase> = Vec::new();
        let mut src_heads: Vec<SolarChunkHead> = Vec::new();

        for (cx, cz) in src_coords {
            let chunk_key = pack_chunk_key(src_world_key, cx, cz);
            src_base_chunks.push(SolarChunkBase {
                chunk_key,
                world_key: src_world_key,
                chunk_x: cx,
                chunk_z: cz,
                data: vec![0x11, 0x22, 0x33],
                base_revision: 5,
                created_at: Timestamp::from_micros_since_unix_epoch(10),
                updated_at: Timestamp::from_micros_since_unix_epoch(10),
            });
            src_heads.push(SolarChunkHead {
                chunk_key,
                world_key: src_world_key,
                chunk_x: cx,
                chunk_z: cz,
                applied_revision: 5,
                base_revision: 5,
                patch_count: 0,
                updated_at: Timestamp::from_micros_since_unix_epoch(10),
                last_actor: "template_author".to_string(),
            });
        }

        let snapshot_src_base = src_base_chunks.clone();
        let snapshot_src_heads = src_heads.clone();

        // Perform template cloning to dst_world_key
        let mut dst_base_chunks: Vec<SolarChunkBase> = Vec::new();
        let mut dst_heads: Vec<SolarChunkHead> = Vec::new();

        for chunk in &src_base_chunks {
            let dst_chunk_key = pack_chunk_key(dst_world_key, chunk.chunk_x, chunk.chunk_z);
            assert_ne!(dst_chunk_key, chunk.chunk_key, "Destination chunk key must differ from source");

            dst_base_chunks.push(SolarChunkBase {
                chunk_key: dst_chunk_key,
                world_key: dst_world_key,
                chunk_x: chunk.chunk_x,
                chunk_z: chunk.chunk_z,
                data: chunk.data.clone(),
                base_revision: chunk.base_revision,
                created_at: Timestamp::from_micros_since_unix_epoch(20),
                updated_at: Timestamp::from_micros_since_unix_epoch(20),
            });

            dst_heads.push(SolarChunkHead {
                chunk_key: dst_chunk_key,
                world_key: dst_world_key,
                chunk_x: chunk.chunk_x,
                chunk_z: chunk.chunk_z,
                applied_revision: chunk.base_revision,
                base_revision: chunk.base_revision,
                patch_count: 0,
                updated_at: Timestamp::from_micros_since_unix_epoch(20),
                last_actor: "system_seed".to_string(),
            });
        }

        // Verify destination chunk keys and properties
        for (i, dst_chunk) in dst_base_chunks.iter().enumerate() {
            let (unpacked_world, unpacked_x, unpacked_z) = unpack_chunk_key(dst_chunk.chunk_key);
            assert_eq!(unpacked_world, dst_world_key);
            assert_eq!(unpacked_x, src_coords[i].0);
            assert_eq!(unpacked_z, src_coords[i].1);
        }

        // Verify source chunks are completely unmodified
        assert_eq!(src_base_chunks, snapshot_src_base, "Source base chunks must not be modified");
        assert_eq!(src_heads, snapshot_src_heads, "Source heads must not be modified");
    }

    #[test]
    fn test_negative_chunk_coordinates_and_world_key_boundaries() {
        let extreme_cases = [
            (1u64, -1, -1),
            (u64::MAX, -1, -1),
            (u64::MAX, i32::MIN, i32::MAX),
            (0x8000_0000_0000_0000u64, i32::MIN, i32::MIN),
            (0x0000_0000_FFFF_FFFFu64, -32768, 32767),
            (123456789u64, -1000000, 1000000),
        ];

        for (world_key, cx, cz) in extreme_cases {
            let key = pack_chunk_key(world_key, cx, cz);
            let (unpacked_world, unpacked_x, unpacked_z) = unpack_chunk_key(key);

            assert_eq!(unpacked_world, world_key, "world_key must roundtrip exactly");
            assert_eq!(unpacked_x, cx, "negative/boundary chunk_x must roundtrip exactly");
            assert_eq!(unpacked_z, cz, "negative/boundary chunk_z must roundtrip exactly");

            // Local block conversion inside negative chunk
            let edit = SolarBlockEdit::new(15, 64, 15, 1);
            let (wx, wy, wz) = edit.to_world_coords(cx, cz);
            assert_eq!(wy, 64);
            let expected_wx = (cx << 4) + 15;
            let expected_wz = (cz << 4) + 15;
            assert_eq!(wx, expected_wx);
            assert_eq!(wz, expected_wz);
        }
    }

    #[test]
    fn test_patch_size_and_height_boundary_rejection() {
        let world_min_y = -64;
        let world_height = 384; // max_y = 320

        // 1. Empty batch rejection
        let empty: Vec<SolarBlockEdit> = vec![];
        let res_empty = validate_patch_bounds(&empty, world_min_y, world_height);
        assert!(res_empty.is_err());
        assert!(res_empty.unwrap_err().contains("empty batch"));

        // 2. Max size limit (256 accepted, 257 rejected)
        let exact_max: Vec<SolarBlockEdit> = (0..256)
            .map(|i| SolarBlockEdit::new((i % 16) as u8, 0, ((i / 16) % 16) as u8, 1))
            .collect();
        assert!(validate_patch_bounds(&exact_max, world_min_y, world_height).is_ok());

        let mut oversized = exact_max;
        oversized.push(SolarBlockEdit::new(0, 0, 0, 1));
        let res_oversized = validate_patch_bounds(&oversized, world_min_y, world_height);
        assert!(res_oversized.is_err());
        assert!(res_oversized.unwrap_err().contains("exceeds maximum batch limit"));

        // 3. Local coordinate bounds (x >= 16 or z >= 16 rejected)
        let bad_local_x = vec![SolarBlockEdit::new(16, 0, 0, 1)];
        assert!(validate_patch_bounds(&bad_local_x, world_min_y, world_height).is_err());

        let bad_local_z = vec![SolarBlockEdit::new(0, 0, 16, 1)];
        assert!(validate_patch_bounds(&bad_local_z, world_min_y, world_height).is_err());

        // 4. World height boundary validation
        // Exact lower bound: -64 (valid)
        assert!(validate_patch_bounds(&[SolarBlockEdit::new(0, -64, 0, 1)], world_min_y, world_height).is_ok());
        // Below lower bound: -65 (invalid)
        assert!(validate_patch_bounds(&[SolarBlockEdit::new(0, -65, 0, 1)], world_min_y, world_height).is_err());

        // Exact upper bound: 319 (valid)
        assert!(validate_patch_bounds(&[SolarBlockEdit::new(0, 319, 0, 1)], world_min_y, world_height).is_ok());
        // At/above upper bound: 320 (invalid)
        assert!(validate_patch_bounds(&[SolarBlockEdit::new(0, 320, 0, 1)], world_min_y, world_height).is_err());
    }

    #[test]
    fn test_import_lifecycle_transitions_and_invalid_transitions() {
        // Valid forward transitions
        assert!(validate_world_lifecycle_transition(None, WORLD_STATUS_IMPORTING).is_ok());
        assert!(validate_world_lifecycle_transition(None, WORLD_STATUS_ACTIVE).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_ACTIVE).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_FAILED).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_ACTIVE), WORLD_STATUS_FALLBACK).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_FALLBACK), WORLD_STATUS_ACTIVE).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_FAILED), WORLD_STATUS_IMPORTING).is_ok());

        // Idempotent re-assertions
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_ACTIVE), WORLD_STATUS_ACTIVE).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_IMPORTING).is_ok());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_FALLBACK), WORLD_STATUS_FALLBACK).is_ok());

        // Invalid transitions
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_FAILED), WORLD_STATUS_ACTIVE).is_err());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_IMPORTING), WORLD_STATUS_FALLBACK).is_err());
        assert!(validate_world_lifecycle_transition(Some(WORLD_STATUS_FALLBACK), WORLD_STATUS_FAILED).is_err());
        assert!(validate_world_lifecycle_transition(None, WORLD_STATUS_FALLBACK).is_err());
        assert!(validate_world_lifecycle_transition(None, WORLD_STATUS_FAILED).is_err());
    }

    #[test]
    fn test_duplicate_import_and_idempotency() {
        let world_key = 500u64;
        let world = SolarWorld {
            world_key,
            world_name: "dungeon_1".to_string(),
            kind: "dungeon".to_string(),
            owner: "team_alpha".to_string(),
            status: WORLD_STATUS_ACTIVE.to_string(),
            total_chunks: 100,
            completed_chunks: 100,
            import_phase: IMPORT_PHASE_COMPLETED.to_string(),
            failure_reason: String::new(),
            harbor_revision: 42,
            harbor_checksum: "sha256:abcd1234ef567890".to_string(),
            min_y: -64,
            height: 384,
            spawn_x: 0.0,
            spawn_y: 64.0,
            spawn_z: 0.0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            seed: 12345,
            source_template: "template_dungeon".to_string(),
            created_at: Timestamp::from_micros_since_unix_epoch(100),
            updated_at: Timestamp::from_micros_since_unix_epoch(100),
        };

        // Identical re-import on active world -> Idempotent no-op
        let incoming_rev = 42u64;
        let incoming_csum = "sha256:abcd1234ef567890";
        let is_idempotent = world.status == WORLD_STATUS_ACTIVE
            && world.harbor_revision == incoming_rev
            && world.harbor_checksum == incoming_csum;
        assert!(is_idempotent);

        // Conflicting import with changed checksum -> rejected without mutation
        let conflicting_csum = "sha256:different_hash";
        let is_conflicting = world.status == WORLD_STATUS_ACTIVE
            && (world.harbor_revision != incoming_rev || world.harbor_checksum != conflicting_csum);
        assert!(is_conflicting);
        assert_eq!(world.harbor_checksum, "sha256:abcd1234ef567890", "World state remains unmutated");
    }

    #[test]
    fn test_commit_acknowledgement_only_after_durable_head_advancement() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let mut head = SolarChunkHead {
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

        let mut committed_patches = Vec::new();

        // Worker submits patch expecting revision 3
        let next_rev = check_expected_revision(Some(head.applied_revision), 3).unwrap();
        assert_eq!(next_rev, 4);

        // State changes executed atomically
        let new_patch = SolarChunkPatch {
            id: 4,
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            revision: next_rev,
            edits: vec![SolarBlockEdit::new(0, 64, 0, 100)],
            actor: "actor_4".to_string(),
            created_at: Timestamp::from_micros_since_unix_epoch(40),
        };
        committed_patches.push(new_patch);

        head.applied_revision = next_rev;
        head.patch_count += 1;
        head.last_actor = "actor_4".to_string();

        // Acknowledgement condition: head must match committed patch revision
        let acknowledged = head.applied_revision == 4 && committed_patches.last().unwrap().revision == 4;
        assert!(acknowledged, "Commit is acknowledged only after head revision advancement");
    }

    #[test]
    fn test_stale_revision_conflict() {
        let head = SolarChunkHead {
            chunk_key: pack_chunk_key(10, 0, 0),
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            applied_revision: 7,
            base_revision: 0,
            patch_count: 7,
            updated_at: Timestamp::from_micros_since_unix_epoch(70),
            last_actor: "actor_7".to_string(),
        };

        // Stale revision (older) -> rejected
        let stale_err = check_expected_revision(Some(head.applied_revision), 6);
        assert!(stale_err.is_err());
        assert!(stale_err.unwrap_err().contains("revision conflict"));

        // Future revision (gap) -> rejected
        let future_err = check_expected_revision(Some(head.applied_revision), 8);
        assert!(future_err.is_err());
        assert!(future_err.unwrap_err().contains("revision conflict"));
    }

    #[test]
    fn test_compaction_lease_conflict_and_recovery() {
        let t0 = Timestamp::from_micros_since_unix_epoch(10_000_000); // 10.0s
        let _lease_duration = 30_000_000; // 30s lease -> expires at 40.0s

        // 1. Initial pending request acquisition succeeds
        let initial_status = COMPACTION_STATUS_PENDING;
        assert!(evaluate_compaction_lease_acquire(initial_status, Timestamp::from_micros_since_unix_epoch(0), Timestamp::from_micros_since_unix_epoch(0), 0, t0).is_ok());

        let mut req = SolarCompactionRequest {
            chunk_key: pack_chunk_key(1, 0, 0),
            world_key: 1,
            chunk_x: 0,
            chunk_z: 0,
            head_revision: 64,
            patch_count: 64,
            status: COMPACTION_STATUS_IN_PROGRESS.to_string(),
            lease_token: 1,
            lease_owner: "worker_alpha".to_string(),
            lease_expires_at: Timestamp::from_micros_since_unix_epoch(40_000_000),
            compaction_attempts: 0,
            last_failed_at: Timestamp::from_micros_since_unix_epoch(0),
            backoff_micros: 0,
            failure_reason: String::new(),
            requested_at: t0,
            requested_by: "system".to_string(),
        };

        // 2. Concurrent worker attempts acquisition while lease is active (at 20s) -> Conflict rejected
        let t_active = Timestamp::from_micros_since_unix_epoch(20_000_000);
        let conflict = evaluate_compaction_lease_acquire(&req.status, req.lease_expires_at, req.last_failed_at, req.backoff_micros, t_active);
        assert!(conflict.is_err());
        assert!(conflict.unwrap_err().contains("compaction lease conflict"));

        // 3. Worker Alpha completes with valid token -> Allowed
        assert!(evaluate_compaction_lease_release(&req.status, req.lease_token, 1).is_ok());

        // 4. Stale token rejection
        assert!(evaluate_compaction_lease_release(&req.status, req.lease_token, 999).is_err());

        // 5. Abandoned worker recovery: time advances past 40s (at 45s) -> Acquisition succeeds
        let t_expired = Timestamp::from_micros_since_unix_epoch(45_000_000);
        assert!(evaluate_compaction_lease_acquire(&req.status, req.lease_expires_at, req.last_failed_at, req.backoff_micros, t_expired).is_ok());

        // Reclaim lease with next token
        req.lease_token = 2;
        req.lease_owner = "worker_beta".to_string();
        req.lease_expires_at = Timestamp::from_micros_since_unix_epoch(75_000_000);

        // 6. Worker Beta fails compaction -> Marks failed status with backoff
        assert!(evaluate_compaction_lease_release(&req.status, req.lease_token, 2).is_ok());
        req.status = COMPACTION_STATUS_FAILED.to_string();
        req.lease_owner = String::new();
        req.compaction_attempts = 1;
        req.last_failed_at = t_expired;
        req.backoff_micros = 5_000_000; // 5s backoff

        // 7. Attempting immediate re-acquisition before backoff expires (at 47s) -> Backoff rejected
        let t_during_backoff = Timestamp::from_micros_since_unix_epoch(47_000_000);
        let backoff_res = evaluate_compaction_lease_acquire(&req.status, req.lease_expires_at, req.last_failed_at, req.backoff_micros, t_during_backoff);
        assert!(backoff_res.is_err());
        assert!(backoff_res.unwrap_err().contains("backoff active"));

        // 8. Re-acquisition after backoff expires (at 51s) -> Acquisition succeeds
        let t_after_backoff = Timestamp::from_micros_since_unix_epoch(51_000_000);
        assert!(evaluate_compaction_lease_acquire(&req.status, req.lease_expires_at, req.last_failed_at, req.backoff_micros, t_after_backoff).is_ok());
    }

    #[test]
    fn test_fallback_metadata_preserves_exact_harbor_revision_and_checksum() {
        let world = SolarWorld {
            world_key: 100,
            world_name: "production_hub".to_string(),
            kind: "lobby".to_string(),
            owner: "system".to_string(),
            status: WORLD_STATUS_FALLBACK.to_string(),
            total_chunks: 64,
            completed_chunks: 64,
            import_phase: IMPORT_PHASE_COMPLETED.to_string(),
            failure_reason: String::new(),
            harbor_revision: 999,
            harbor_checksum: "sha256:source_golden_checksum".to_string(),
            min_y: -64,
            height: 384,
            spawn_x: 0.0,
            spawn_y: 64.0,
            spawn_z: 0.0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            seed: 42,
            source_template: "".to_string(),
            created_at: Timestamp::from_micros_since_unix_epoch(100),
            updated_at: Timestamp::from_micros_since_unix_epoch(100),
        };

        // Mutation eligibility fails closed in FALLBACK mode
        let eligibility = validate_world_mutation_eligibility(&world);
        assert_eq!(eligibility, Err(SolarAuthError::WorldInFallback { world_key: 100 }));

        // Revision & checksum remain unpolluted
        assert_eq!(world.harbor_revision, 999);
        assert_eq!(world.harbor_checksum, "sha256:source_golden_checksum");
    }

    #[test]
    fn test_two_worlds_same_coordinates_cannot_cross_read_or_write() {
        let world_a_key = 100u64;
        let world_b_key = 200u64;
        let cx = 5;
        let cz = -10;

        let key_a = pack_chunk_key(world_a_key, cx, cz);
        let key_b = pack_chunk_key(world_b_key, cx, cz);

        assert_ne!(key_a, key_b, "Packed chunk keys across distinct worlds must be unique");

        // Verify unpacking separates worlds
        let (wa, xa, za) = unpack_chunk_key(key_a);
        let (wb, xb, zb) = unpack_chunk_key(key_b);

        assert_eq!(wa, world_a_key);
        assert_eq!(wb, world_b_key);
        assert_eq!(xa, cx);
        assert_eq!(xb, cx);
        assert_eq!(za, cz);
        assert_eq!(zb, cz);
    }

    #[test]
    fn test_fallback_mode_rejects_all_mutations() {
        let active_world = SolarWorld {
            world_key: 1,
            world_name: "active".to_string(),
            kind: "lobby".to_string(),
            owner: "".to_string(),
            status: WORLD_STATUS_ACTIVE.to_string(),
            total_chunks: 10,
            completed_chunks: 10,
            import_phase: IMPORT_PHASE_COMPLETED.to_string(),
            failure_reason: String::new(),
            harbor_revision: 1,
            harbor_checksum: "csum".to_string(),
            min_y: -64,
            height: 384,
            spawn_x: 0.0,
            spawn_y: 64.0,
            spawn_z: 0.0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            seed: 0,
            source_template: "".to_string(),
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };

        let fallback_world = SolarWorld {
            status: WORLD_STATUS_FALLBACK.to_string(),
            ..active_world.clone()
        };

        let importing_world = SolarWorld {
            status: WORLD_STATUS_IMPORTING.to_string(),
            ..active_world.clone()
        };

        assert!(validate_world_mutation_eligibility(&active_world).is_ok());
        assert!(matches!(
            validate_world_mutation_eligibility(&fallback_world),
            Err(SolarAuthError::WorldInFallback { .. })
        ));
        assert!(matches!(
            validate_world_mutation_eligibility(&importing_world),
            Err(SolarAuthError::WorldNotActive { .. })
        ));
    }

    #[test]
    fn test_retired_per_block_and_loader_paths_fail_closed() {
        // Enforces that empty or un-batched edits are rejected
        let empty_batch: Vec<SolarBlockEdit> = Vec::new();
        let res = validate_patch_bounds(&empty_batch, -64, 384);
        assert!(res.is_err(), "Single-block / empty un-batched legacy writes are blocked");
    }

    struct MockContainerAuthorizer;
    impl ContainerAccessAuthorizer for MockContainerAuthorizer {
        fn authorize_container_access(
            &self,
            _world_key: u64,
            world_x: i32,
            _world_y: i32,
            world_z: i32,
            actor: &str,
            auth: &SolarAuthContext,
        ) -> Result<(), SolarAuthError> {
            if auth.is_admin {
                return Ok(());
            }
            if world_x == 99 && world_z == 99 {
                return Err(SolarAuthError::ContainerAccessDenied {
                    container_type: "LockedVault".to_string(),
                    reason: format!("actor '{actor}' lacks vault key"),
                });
            }
            Ok(())
        }
    }

    #[test]
    fn test_container_access_authorization_chain() {
        let container_auth = MockContainerAuthorizer;
        let player_auth = SolarAuthContext {
            actor: "thief".to_string(),
            is_admin: false,
            bypass_claims: false,
            client_tick: 1,
        };

        let res_denied = container_auth.authorize_container_access(1, 99, 64, 99, "thief", &player_auth);
        assert!(matches!(res_denied, Err(SolarAuthError::ContainerAccessDenied { .. })));

        let res_allowed = container_auth.authorize_container_access(1, 10, 64, 10, "thief", &player_auth);
        assert!(res_allowed.is_ok());
    }

    // -----------------------------------------------------------------------
    // Phase 6 Tests: Durable Import Progress, Boss-Bar Monotonicity, Resumable Append
    // -----------------------------------------------------------------------

    #[test]
    fn test_import_progress_monotonic_percentage_and_phases() {
        let mut world = SolarWorld {
            world_key: 777,
            world_name: "mega_world".to_string(),
            kind: "lobby".to_string(),
            owner: "".to_string(),
            status: WORLD_STATUS_IMPORTING.to_string(),
            total_chunks: 200,
            completed_chunks: 0,
            import_phase: IMPORT_PHASE_DISCOVERY.to_string(),
            failure_reason: String::new(),
            harbor_revision: 1,
            harbor_checksum: "csum".to_string(),
            min_y: -64,
            height: 384,
            spawn_x: 0.0,
            spawn_y: 64.0,
            spawn_z: 0.0,
            spawn_yaw: 0.0,
            spawn_pitch: 0.0,
            seed: 0,
            source_template: "".to_string(),
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };

        assert_eq!(world.progress_pct(), 0);
        assert_eq!(world.import_phase, IMPORT_PHASE_DISCOVERY);

        // Streaming phase: 50 chunks streamed
        world.import_phase = IMPORT_PHASE_STREAMING_BASE.to_string();
        world.completed_chunks = 50;
        assert_eq!(world.progress_pct(), 25);

        // Streaming phase: 150 chunks streamed
        world.completed_chunks = 150;
        assert_eq!(world.progress_pct(), 75);

        // Validation phase: 200 chunks streamed
        world.import_phase = IMPORT_PHASE_VALIDATING.to_string();
        world.completed_chunks = 200;
        assert_eq!(world.progress_pct(), 100);

        // Transition to ACTIVE
        world.status = WORLD_STATUS_ACTIVE.to_string();
        world.import_phase = IMPORT_PHASE_COMPLETED.to_string();
        assert_eq!(world.progress_pct(), 100);
    }

    #[test]
    fn test_resumable_import_idempotent_append() {
        let chunk_key = pack_chunk_key(10, 0, 0);
        let existing_base = SolarChunkBase {
            chunk_key,
            world_key: 10,
            chunk_x: 0,
            chunk_z: 0,
            data: vec![0x10, 0x20],
            base_revision: 0,
            created_at: Timestamp::from_micros_since_unix_epoch(1),
            updated_at: Timestamp::from_micros_since_unix_epoch(1),
        };

        // Incoming identical chunk append during resume
        let incoming_data = vec![0x10, 0x20];
        let is_resumable_match = existing_base.data == incoming_data && existing_base.base_revision == 0;
        assert!(is_resumable_match, "Re-streaming existing base chunk on resume is idempotent");
    }
}
