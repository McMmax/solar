//! Solar native bridge — pure functions over a live SpacetimeDB `DbConnection`.
//!
//! This crate owns no connection of its own. Per [ROADMAP.md S1][roadmap], Stomzy's
//! native bridge (`minestom_stdb_bridge`, `platforms/stomzy/spacetime/native`) already
//! owns the one `DbConnection` per server process; its `hub.rs` adds thin
//! `#[ffi_export]` wrappers that call into this crate, passing it that live connection.
//! This crate has no FFI surface itself — Panama/jextract binds `hub.rs`, not this.
//!
//! # Solar v1 Persistence Architecture
//! - Single-column `u128 chunk_key` primary keys enable direct O(1) in-memory SDK cache lookups.
//! - Replaces linear scan over chunk rows and unbatched delta log rows.
//! - Bounded edit batches (`solar_chunk_patch`) with atomic expected-revision verification.
//!
//! [roadmap]: ../../docs/ROADMAP.md

pub use solar_schema::{
    pack_chunk_key, unpack_chunk_key, SolarAuthContext, SolarAuthError, SolarBlockEdit,
    COMPACTION_THRESHOLD_PATCHES, MAX_EDITS_PER_PATCH,
};

use minestom_stdb::{
    backend_server_link_native_identity_reducer::backend_server_link_native_identity as BackendServerLinkNativeIdentityExt,
    DbConnection, SubscriptionHandle,
};
use spacetimedb_sdk::DbContext;

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
    conn.reducers.backend_server_link_native_identity(server_id).is_ok()
}
