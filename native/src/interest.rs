//! Tile-based interest management, ref-counting, hysteresis, and priority calculations.

use std::collections::{HashMap, HashSet};
use solar_schema::pack_chunk_key;

/// Chunk-to-tile size: 8 chunks x 8 chunks = 128 x 128 block columns.
pub const TILE_SIZE_CHUNKS: i32 = 8;

/// Converts chunk column coordinates `(chunk_x, chunk_z)` to tile coordinates `(tile_x, tile_z)`.
#[inline]
pub fn chunk_to_tile_coords(chunk_x: i32, chunk_z: i32) -> (i32, i32) {
    (chunk_x.div_euclid(TILE_SIZE_CHUNKS), chunk_z.div_euclid(TILE_SIZE_CHUNKS))
}

/// Converts tile coordinates `(tile_x, tile_z)` to bounding chunk column range `(min_cx, max_cx, min_cz, max_cz)` inclusive.
#[inline]
pub fn tile_to_chunk_bounds(tile_x: i32, tile_z: i32) -> (i32, i32, i32, i32) {
    let min_cx = tile_x * TILE_SIZE_CHUNKS;
    let max_cx = min_cx + (TILE_SIZE_CHUNKS - 1);
    let min_cz = tile_z * TILE_SIZE_CHUNKS;
    let max_cz = min_cz + (TILE_SIZE_CHUNKS - 1);
    (min_cx, max_cx, min_cz, max_cz)
}

/// Generates all 64 packed `u128 chunk_key` identifiers contained within an 8x8 tile.
pub fn tile_chunk_keys(world_key: u64, tile_x: i32, tile_z: i32) -> Vec<u128> {
    let (min_cx, max_cx, min_cz, max_cz) = tile_to_chunk_bounds(tile_x, tile_z);
    let mut keys = Vec::with_capacity((TILE_SIZE_CHUNKS * TILE_SIZE_CHUNKS) as usize);
    for cx in min_cx..=max_cx {
        for cz in min_cz..=max_cz {
            keys.push(pack_chunk_key(world_key, cx, cz));
        }
    }
    keys
}

/// Ref-counted tracker for active 8x8 chunk tiles within a world.
///
/// Ensures:
/// - Multiple players viewing the same tile share a single underlying interest subscription.
/// - Unsubscribing a player decrements the reference count without dropping the tile if others remain.
/// - Full cleanup and deactivation occurs only when the final subscriber leaves.
#[derive(Default, Debug, Clone)]
pub struct SolarTileTracker {
    pub world_key: u64,
    /// Reference count for each active tile coordinate `(tile_x, tile_z)`.
    tile_refs: HashMap<(i32, i32), usize>,
}

impl SolarTileTracker {
    pub fn new(world_key: u64) -> Self {
        Self {
            world_key,
            tile_refs: HashMap::new(),
        }
    }

    /// Subscribes to a tile.
    /// Returns `true` if this is the FIRST reference (initiates activation/pinning).
    /// Returns `false` if the tile was already active (increments ref count only).
    pub fn subscribe_tile(&mut self, tile_x: i32, tile_z: i32) -> bool {
        let count = self.tile_refs.entry((tile_x, tile_z)).or_insert(0);
        let was_new = *count == 0;
        *count += 1;
        was_new
    }

    /// Unsubscribes from a tile.
    /// Returns `true` if the reference count reached 0 and the tile was deactivated/cleaned up.
    /// Returns `false` if other subscribers still reference this tile or if it was not present.
    pub fn unsubscribe_tile(&mut self, tile_x: i32, tile_z: i32) -> bool {
        if let Some(count) = self.tile_refs.get_mut(&(tile_x, tile_z)) {
            if *count > 1 {
                *count -= 1;
                false
            } else {
                self.tile_refs.remove(&(tile_x, tile_z));
                true
            }
        } else {
            false
        }
    }

    /// Returns the current subscriber count for a tile coordinate.
    pub fn subscriber_count(&self, tile_x: i32, tile_z: i32) -> usize {
        self.tile_refs.get(&(tile_x, tile_z)).copied().unwrap_or(0)
    }

    /// Returns `true` if the tile currently has at least one active subscriber.
    pub fn is_tile_active(&self, tile_x: i32, tile_z: i32) -> bool {
        self.tile_refs.contains_key(&(tile_x, tile_z))
    }

    /// Returns total active tile count.
    pub fn active_tile_count(&self) -> usize {
        self.tile_refs.len()
    }

    /// Clears all tile references.
    pub fn clear(&mut self) {
        self.tile_refs.clear();
    }
}

/// Computes desired tile subscription interest with a two-chunk hysteresis margin.
///
/// - `load_tiles`: Tiles covering the player's core view radius.
/// - `retention_tiles`: Expanded envelope including the `hysteresis_margin_chunks` buffer.
///   Tiles outside this boundary are evicted when moving away.
pub fn compute_tile_interest_with_hysteresis(
    player_chunk_x: i32,
    player_chunk_z: i32,
    view_distance_chunks: i32,
    hysteresis_chunks: i32,
) -> (HashSet<(i32, i32)>, HashSet<(i32, i32)>) {
    let mut load_tiles = HashSet::new();
    let mut retention_tiles = HashSet::new();

    let load_min_cx = player_chunk_x - view_distance_chunks;
    let load_max_cx = player_chunk_x + view_distance_chunks;
    let load_min_cz = player_chunk_z - view_distance_chunks;
    let load_max_cz = player_chunk_z + view_distance_chunks;

    let retain_distance = view_distance_chunks + hysteresis_chunks;
    let retain_min_cx = player_chunk_x - retain_distance;
    let retain_max_cx = player_chunk_x + retain_distance;
    let retain_min_cz = player_chunk_z - retain_distance;
    let retain_max_cz = player_chunk_z + retain_distance;

    for cx in (load_min_cx..=load_max_cx).step_by(TILE_SIZE_CHUNKS as usize) {
        for cz in (load_min_cz..=load_max_cz).step_by(TILE_SIZE_CHUNKS as usize) {
            load_tiles.insert(chunk_to_tile_coords(cx, cz));
        }
    }
    load_tiles.insert(chunk_to_tile_coords(load_max_cx, load_max_cz));

    for cx in (retain_min_cx..=retain_max_cx).step_by(TILE_SIZE_CHUNKS as usize) {
        for cz in (retain_min_cz..=retain_max_cz).step_by(TILE_SIZE_CHUNKS as usize) {
            retention_tiles.insert(chunk_to_tile_coords(cx, cz));
        }
    }
    retention_tiles.insert(chunk_to_tile_coords(retain_max_cx, retain_max_cz));

    (load_tiles, retention_tiles)
}

/// Pure Rust nearest-first priority calculation based on Euclidean distance.
/// Lower score indicates higher loading priority.
#[inline]
pub fn calculate_nearest_priority(
    chunk_x: i32,
    chunk_z: i32,
    player_chunk_x: i32,
    player_chunk_z: i32,
) -> f32 {
    let dx = (chunk_x - player_chunk_x) as f32;
    let dz = (chunk_z - player_chunk_z) as f32;
    (dx * dx + dz * dz).sqrt()
}

/// Pure Rust movement-predicted priority calculation.
///
/// Extrapolates player chunk position along their velocity vector over `lookahead_seconds`,
/// biasing chunk priority forward into the player's movement trajectory.
pub fn calculate_movement_predicted_priority(
    chunk_x: i32,
    chunk_z: i32,
    player_chunk_x: i32,
    player_chunk_z: i32,
    velocity_x: f32,
    velocity_z: f32,
    lookahead_seconds: f32,
) -> f32 {
    let current_dist = calculate_nearest_priority(chunk_x, chunk_z, player_chunk_x, player_chunk_z);

    let pred_px = (player_chunk_x as f32) + (velocity_x * lookahead_seconds);
    let pred_pz = (player_chunk_z as f32) + (velocity_z * lookahead_seconds);
    let dx = (chunk_x as f32) - pred_px;
    let dz = (chunk_z as f32) - pred_pz;
    let predicted_dist = (dx * dx + dz * dz).sqrt();

    // 35% weight on current distance + 65% weight on velocity-predicted position
    (0.35 * current_dist) + (0.65 * predicted_dist)
}
