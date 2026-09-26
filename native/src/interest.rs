//! Tile-based interest management, ref-counting, hysteresis, and priority calculations.

use solar_schema::pack_chunk_key;
use std::collections::{HashMap, HashSet};

/// Chunk-to-tile size: 8 chunks x 8 chunks = 128 x 128 block columns.
pub const TILE_SIZE_CHUNKS: i32 = 8;

/// Converts chunk column coordinates `(chunk_x, chunk_z)` to tile coordinates `(tile_x, tile_z)`.
#[inline]
pub fn chunk_to_tile_coords(chunk_x: i32, chunk_z: i32) -> (i32, i32) {
    (
        chunk_x.div_euclid(TILE_SIZE_CHUNKS),
        chunk_z.div_euclid(TILE_SIZE_CHUNKS),
    )
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
#[allow(clippy::type_complexity)]
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

    let (min_tx, min_tz) = chunk_to_tile_coords(load_min_cx, load_min_cz);
    let (max_tx, max_tz) = chunk_to_tile_coords(load_max_cx, load_max_cz);
    for tx in min_tx..=max_tx {
        for tz in min_tz..=max_tz {
            load_tiles.insert((tx, tz));
        }
    }

    let (r_min_tx, r_min_tz) = chunk_to_tile_coords(retain_min_cx, retain_min_cz);
    let (r_max_tx, r_max_tz) = chunk_to_tile_coords(retain_max_cx, retain_max_cz);
    for tx in r_min_tx..=r_max_tx {
        for tz in r_min_tz..=r_max_tz {
            retention_tiles.insert((tx, tz));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_8_16_32_interest_and_bounds() {
        for &view in &[8, 16, 32] {
            let (load_tiles, retain_tiles) = compute_tile_interest_with_hysteresis(0, 0, view, 2);
            assert!(!load_tiles.is_empty());
            assert!(!retain_tiles.is_empty());
            assert!(retain_tiles.len() >= load_tiles.len());

            // Check that center tile (0, 0) is always included
            assert!(load_tiles.contains(&(0, 0)));
            assert!(retain_tiles.contains(&(0, 0)));

            // Verify bounds for all load tiles
            for &(tx, tz) in &load_tiles {
                let (min_cx, max_cx, min_cz, max_cz) = tile_to_chunk_bounds(tx, tz);
                assert_eq!(max_cx - min_cx, TILE_SIZE_CHUNKS - 1);
                assert_eq!(max_cz - min_cz, TILE_SIZE_CHUNKS - 1);

                let keys = tile_chunk_keys(42, tx, tz);
                assert_eq!(keys.len(), 64);
            }
        }
    }

    #[test]
    fn test_nearest_first_deterministic() {
        let px = 0;
        let pz = 0;

        let mut chunks = vec![
            (10, 10),
            (0, 1),
            (0, 0),
            (5, 5),
            (-2, 0),
            (0, 2),
        ];

        // Sort by nearest priority score
        chunks.sort_by(|&(ax, az), &(bx, bz)| {
            let pa = calculate_nearest_priority(ax, az, px, pz);
            let pb = calculate_nearest_priority(bx, bz, px, pz);
            pa.partial_cmp(&pb).unwrap().then_with(|| (ax, az).cmp(&(bx, bz)))
        });

        assert_eq!(chunks[0], (0, 0)); // dist 0
        assert_eq!(chunks[1], (0, 1)); // dist 1
        assert_eq!(chunks[2], (-2, 0)); // dist 2
        assert_eq!(chunks[3], (0, 2)); // dist 2 (tie-broken deterministically)
        assert_eq!(chunks[4], (5, 5)); // dist ~7.07
        assert_eq!(chunks[5], (10, 10)); // dist ~14.14
    }

    #[test]
    fn test_hysteresis_avoids_churn() {
        // Player starts at chunk (0, 0), view 8, hysteresis 2
        let (_, retain_initial) = compute_tile_interest_with_hysteresis(0, 0, 8, 2);

        // Player makes minor movement within hysteresis boundary (to chunk (1, 0))
        let (load_moved, retain_moved) = compute_tile_interest_with_hysteresis(1, 0, 8, 2);

        // Retention set at (0, 0) should fully contain load_moved so no churn occurs
        for tile in &load_moved {
            assert!(
                retain_initial.contains(tile),
                "Tile {:?} in new load set must be in initial retention set to prevent churn",
                tile
            );
        }

        // Overlap between retention sets should be large
        let common = retain_initial.intersection(&retain_moved).count();
        assert!(common >= retain_initial.len() - 2);
    }

    #[test]
    fn test_duplicate_requests_and_ref_counting() {
        let mut tracker = SolarTileTracker::new(1);

        // Multiple subscribers to tile (2, 3)
        assert!(tracker.subscribe_tile(2, 3)); // 1st: true
        assert!(!tracker.subscribe_tile(2, 3)); // 2nd: false (suppressed duplicate)
        assert!(!tracker.subscribe_tile(2, 3)); // 3rd: false
        assert_eq!(tracker.subscriber_count(2, 3), 3);
        assert!(tracker.is_tile_active(2, 3));
        assert_eq!(tracker.active_tile_count(), 1);

        // First two unsubscriptions do NOT deactivate the tile
        assert!(!tracker.unsubscribe_tile(2, 3));
        assert_eq!(tracker.subscriber_count(2, 3), 2);
        assert!(!tracker.unsubscribe_tile(2, 3));
        assert_eq!(tracker.subscriber_count(2, 3), 1);
        assert!(tracker.is_tile_active(2, 3));

        // Final unsubscription cleans up and returns true
        assert!(tracker.unsubscribe_tile(2, 3));
        assert_eq!(tracker.subscriber_count(2, 3), 0);
        assert!(!tracker.is_tile_active(2, 3));
        assert_eq!(tracker.active_tile_count(), 0);
    }

    #[test]
    fn test_disconnect_clears_state() {
        let mut tracker = SolarTileTracker::new(1);
        tracker.subscribe_tile(0, 0);
        tracker.subscribe_tile(1, 1);
        tracker.subscribe_tile(2, 2);
        assert_eq!(tracker.active_tile_count(), 3);

        tracker.clear();
        assert_eq!(tracker.active_tile_count(), 0);
        assert!(!tracker.is_tile_active(0, 0));
        assert_eq!(tracker.subscriber_count(0, 0), 0);
    }
}
