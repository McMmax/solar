package dev.terrorify.solar;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

/**
 * The SpacetimeDB-facing half of Solar, injected into {@link SolarLoader}. Mirrors
 * {@code dev.terrorify.polar.WorldListener}'s role: this library ({@code :solar}) owns the
 * codec/{@code ChunkLoader} and stays dependency-free of the native bridge; {@code :spacetime}
 * (which already owns the one native connection — see {@code libraries/solar/docs/ARCHITECTURE.md}
 * "one connection per server") implements this interface and does the actual FFI calls.
 *
 * <p>{@link #getChunk} is a synchronous in-process cache read, never a network round-trip —
 * see OPTIMIZATION.md §4. The write methods are fire-and-forget ("oneway"): callers do not
 * wait for reducer acknowledgement.
 */
public interface SolarStore {
    /** Cache read for one chunk column's Solar bytes, or {@code null} if not resident yet. */
    byte @Nullable [] getChunk(@NotNull String worldId, int chunkX, int chunkZ);

    /** Upsert the canonical state of one chunk column ({@code solar_chunk_put}, oneway). */
    void putChunk(@NotNull String worldId, int chunkX, int chunkZ, byte @NotNull [] data);

    /** Append one live block-edit delta ({@code solar_block_set_apply}, oneway). */
    void applyBlockSet(@NotNull String worldId, int x, int y, int z, int state, @NotNull String actor);

    /** Drop deltas already folded into {@code solar_chunk} ({@code solar_block_prune}, oneway). */
    void prune(@NotNull String worldId, int chunkX, int chunkZ, long upToId);

    /** Create or update a world's metadata row ({@code solar_world_upsert}, oneway). */
    void putWorld(@NotNull SolarWorld world);

    /** No-op implementation for loaders that don't need Solar persistence (e.g. tests). */
    SolarStore NO_OP = new SolarStore() {
        @Override public byte @Nullable [] getChunk(@NotNull String worldId, int chunkX, int chunkZ) { return null; }
        @Override public void putChunk(@NotNull String worldId, int chunkX, int chunkZ, byte @NotNull [] data) {}
        @Override public void applyBlockSet(@NotNull String worldId, int x, int y, int z, int state, @NotNull String actor) {}
        @Override public void prune(@NotNull String worldId, int chunkX, int chunkZ, long upToId) {}
        @Override public void putWorld(@NotNull SolarWorld world) {}
    };
}
