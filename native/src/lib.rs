//! Solar native bridge — pure functions over a live SpacetimeDB `DbConnection`.
//!
//! This crate owns no connection of its own. Per [ROADMAP.md S1][roadmap], Stomzy's
//! native bridge (`minestom_stdb_bridge`, `platforms/stomzy/spacetime/native`) already
//! owns the one `DbConnection` per server process; its `hub.rs` adds thin
//! `#[ffi_export]` wrappers that call into this crate, passing it that live connection.
//! This crate has no FFI surface itself — Panama/jextract binds `hub.rs`, not this.
//!
//! [roadmap]: ../../docs/ROADMAP.md

use minestom_stdb::{
    backend_server_link_native_identity_reducer::backend_server_link_native_identity as BackendServerLinkNativeIdentityExt,
    solar_block_prune_reducer::solar_block_prune as SolarBlockPruneExt,
    solar_block_set_apply_reducer::solar_block_set_apply as SolarBlockSetApplyExt,
    solar_chunk_put_reducer::solar_chunk_put as SolarChunkPutExt,
    solar_chunk_table::SolarChunkTableAccess,
    solar_seed_from_template_reducer::solar_seed_from_template as SolarSeedFromTemplateExt,
    solar_world_upsert_reducer::solar_world_upsert as SolarWorldUpsertExt,
    DbConnection, SubscriptionHandle,
};
use spacetimedb_sdk::{DbContext, Table};

/// Cache read: the canonical `data` blob for one chunk column, if the client's
/// subscription cache currently holds it. No network round-trip — reads whatever
/// `subscribe_world` (or the module's default subscription set) has already synced.
///
/// `solar_chunk` has no client-side index handle for its `(world_id, chunk_x, chunk_z)`
/// btree (only the `id` unique constraint gets one — confirmed against the generated
/// bindings), so this is a linear scan of the cache. Fine at Solar's target scale
/// (single-digit-KB rows, a server's cache holds only its subscribed worlds — see
/// OPTIMIZATION.md §3); revisit if a world's resident chunk count grows large.
pub fn get_world_chunk(conn: &DbConnection, world_id: &str, chunk_x: i32, chunk_z: i32) -> Option<Vec<u8>> {
    conn.db
        .solar_chunk()
        .iter()
        .find(|c| c.world_id == world_id && c.chunk_x == chunk_x && c.chunk_z == chunk_z)
        .map(|c| c.data)
}

/// Upsert the canonical state of one chunk column (`solar_chunk_put`). Oneway —
/// fire-and-forget, matching every other write in this bridge (never await on the tick).
pub fn put_world_chunk(conn: &DbConnection, world_id: String, chunk_x: i32, chunk_z: i32, data: Vec<u8>) -> bool {
    conn.reducers.solar_chunk_put(world_id, chunk_x, chunk_z, data).is_ok()
}

/// Append one live block-edit delta (`solar_block_set_apply`). Chunk coords are
/// derived server-side (`x>>4`, `z>>4`) — not passed here.
pub fn apply_block_set(conn: &DbConnection, world_id: String, x: i32, y: i32, z: i32, state: i32, actor: String) -> bool {
    conn.reducers.solar_block_set_apply(world_id, x, y, z, state, actor).is_ok()
}

/// Drop deltas already folded into `solar_chunk` (`solar_block_prune`). Call only
/// after the corresponding `put_world_chunk` succeeds — the two reducers are not
/// atomic (see ROADMAP.md S3 / OPTIMIZATION.md §2).
pub fn prune(conn: &DbConnection, world_id: String, chunk_x: i32, chunk_z: i32, up_to_id: u64) -> bool {
    conn.reducers.solar_block_prune(world_id, chunk_x, chunk_z, up_to_id).is_ok()
}

/// Create or update a world's metadata row (`solar_world_upsert`).
#[allow(clippy::too_many_arguments)]
pub fn put_world(
    conn: &DbConnection,
    world_id: String,
    kind: String,
    owner: String,
    min_y: i32,
    height: i32,
    spawn_x: f64,
    spawn_y: f64,
    spawn_z: f64,
    spawn_yaw: f32,
    spawn_pitch: f32,
    seed: i64,
    source_template: String,
) -> bool {
    conn.reducers
        .solar_world_upsert(
            world_id,
            kind,
            owner,
            min_y,
            height,
            spawn_x,
            spawn_y,
            spawn_z,
            spawn_yaw,
            spawn_pitch,
            seed,
            source_template,
        )
        .is_ok()
}

/// Seed a new world by copying a template world's chunks + metadata (`solar_seed_from_template`).
pub fn seed_from_template(conn: &DbConnection, src_world_id: String, dst_world_id: String, owner: String) -> bool {
    conn.reducers.solar_seed_from_template(src_world_id, dst_world_id, owner).is_ok()
}

/// Link this connection's identity as the caller's native-bridge identity
/// (`backend_server_link_native_identity`) — must be called once after connect so the
/// Solar reducers' `assert_backend_server` auth gate accepts calls made over this
/// connection (see `spacetime/coldmain/src/auth.rs`; the native connection's identity
/// is NOT the same as the HTTP path's `SPACETIME_TOKEN` identity).
pub fn link_native_identity(conn: &DbConnection, server_id: String) -> bool {
    conn.reducers.backend_server_link_native_identity(server_id).is_ok()
}

/// Subscribe to one world's `solar_chunk` + `solar_block_set` rows. Per OPTIMIZATION.md
/// §3: dynamic per-world subscriptions, not `SELECT * FROM solar_chunk` — a server's
/// cache should only ever hold the worlds it's actually running. Caller owns the
/// returned handle and must call `.unsubscribe()` (or `_then`) on world unload.
pub fn subscribe_world(conn: &DbConnection, world_id: &str) -> SubscriptionHandle {
    let world_id = world_id.replace('\'', "''");
    conn.subscription_builder().subscribe(vec![
        format!("SELECT * FROM solar_chunk WHERE world_id = '{world_id}'"),
        format!("SELECT * FROM solar_block_set WHERE world_id = '{world_id}'"),
    ])
}
