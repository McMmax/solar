//! Solar — SpacetimeDB world-store schema (the per-chunk sibling to Polar).
//!
//! Polar stores a world as one packed, whole-world file with no random chunk
//! access. Solar instead stores a world as **per-chunk rows** so a JVM server can
//! stream only the chunks it needs from SpacetimeDB, render them, and write block
//! edits back — no world file on disk.
//!
//! This crate is the **single source of truth** for the Solar table contract. An
//! adopting SpacetimeDB module depends on it and re-exports these `#[table]`
//! structs from its own `tables` module so the macros register them into the
//! module's WASM binary (same pattern as `hyperscope-schema`):
//!
//! ```ignore
//! // <your-module>/src/tables/solar.rs
//! pub use solar_schema::{
//!     solar_chunk, solar_block_set, solar_world,
//!     SolarChunk, SolarBlockSet, SolarWorld,
//! };
//! ```
//!
//! Reducers (the put / block-set / compact / seed logic) are **not** part of this
//! crate — SpacetimeDB requires reducers to compile into the module crate, and
//! they carry host concerns (auth/audit). They live in the module's
//! `reducers::solar` and operate on these tables.
//!
//! Design notes:
//!   * SpacetimeDB is the **live bus + store**, in RAM, with a 32 MiB WebSocket
//!     message ceiling. Per-chunk rows are palette-compressed and tiny, so they
//!     sync over subscriptions (unlike a whole-world blob, which does not).
//!   * `solar_chunk` is the canonical state (one row per chunk column). Live
//!     edits ride `solar_block_set` (a small delta log, like Clockwork's
//!     `StdbBlockSetUpdate`) so peers converge without re-reading whole chunks;
//!     deltas are periodically folded back into `solar_chunk` and pruned.
//!   * The `data` blob is the Solar per-chunk byte format (see `FORMAT.md`),
//!     produced/consumed JVM-side from Minestom `Palette` — opaque to the module.

use spacetimedb::{table, Timestamp};

/// Canonical state of one chunk column — exactly one row per
/// `(world_id, chunk_x, chunk_z)` (uniqueness enforced by the upsert reducer,
/// since SpacetimeDB primary keys are single-column). `data` is the Solar
/// per-chunk serialization; `rev` bumps on every write so peers can detect
/// staleness.
#[derive(Clone)]
#[table(
    accessor = solar_chunk,
    public,
    index(accessor = solar_chunk_loc, btree(columns = [world_id, chunk_x, chunk_z])),
)]
pub struct SolarChunk {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub world_id: String,
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// Solar per-chunk byte format (sections' block + biome palettes). Opaque here.
    pub data: Vec<u8>,
    /// Monotonic revision, bumped on each `solar_chunk_put`.
    pub rev: u64,
    pub updated_at: Timestamp,
}

/// Append-only block-edit delta log. Each row is a single block change; peers
/// subscribed to the world apply it live, then it is folded into `solar_chunk`
/// and pruned (`solar_block_prune`). `chunk_x/chunk_z` are derived from `x/z`
/// (>>4) so deltas are indexed per chunk for compaction.
#[derive(Clone)]
#[table(
    accessor = solar_block_set,
    public,
    index(accessor = solar_block_loc, btree(columns = [world_id, chunk_x, chunk_z])),
)]
pub struct SolarBlockSet {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub world_id: String,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Minestom block state id of the new block at (x,y,z).
    pub state: i32,
    /// Who made the edit (player uuid / system), for audit + the future anticheat.
    pub actor: String,
    pub at: Timestamp,
}

/// Per-world metadata: kind (lobby/practice_template/island), owner (team for
/// islands), height bounds, spawn, seed, and the template it was seeded from.
#[derive(Clone)]
#[table(accessor = solar_world, public)]
pub struct SolarWorld {
    #[primary_key]
    pub world_id: String,
    /// "lobby" | "practice_template" | "island" | …
    pub kind: String,
    /// Team / owner id for per-team worlds; empty for shared worlds.
    pub owner: String,
    pub min_y: i32,
    pub height: i32,
    pub spawn_x: f64,
    pub spawn_y: f64,
    pub spawn_z: f64,
    pub spawn_yaw: f32,
    pub spawn_pitch: f32,
    pub seed: i64,
    /// world_id this was seeded from (empty if authored/imported directly).
    pub source_template: String,
    pub created_at: Timestamp,
}
