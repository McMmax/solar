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
    /** Cache read for one chunk column's Solar bytes. Retired in favor of active Hot v2 native frame cache. */
    @Deprecated
    default byte @Nullable [] getChunk(@NotNull String worldId, int chunkX, int chunkZ) {
        throw new UnsupportedOperationException("Materializing getChunk is retired — use Hot v2 native frame cache");
    }

    /** Upsert one chunk column ({@code solar_chunk_put}, oneway). Retired in favor of {@code solar_chunk_base_commit}. */
    @Deprecated
    default void putChunk(@NotNull String worldId, int chunkX, int chunkZ, byte @NotNull [] data) {
        throw new UnsupportedOperationException("solar_chunk_put is retired — use solar_chunk_base_commit");
    }

    /** Append block edit ({@code solar_block_set_apply}, oneway). Retired in favor of {@code solar_commit_edits}. */
    @Deprecated
    default void applyBlockSet(@NotNull String worldId, int x, int y, int z, int state, @NotNull String actor) {
        throw new UnsupportedOperationException("solar_block_set_apply is retired — use solar_commit_edits");
    }

    /** Drop deltas ({@code solar_block_prune}, oneway). Retired in favor of compaction leasing. */
    @Deprecated
    default void prune(@NotNull String worldId, int chunkX, int chunkZ, long upToId) {
        throw new UnsupportedOperationException("solar_block_prune is retired — use compaction leasing");
    }

    /** Create/update world metadata ({@code solar_world_upsert}, oneway). Retired in favor of {@code solar_world_create_or_import}. */
    @Deprecated
    default void putWorld(@NotNull SolarWorld world) {
        throw new UnsupportedOperationException("solar_world_upsert is retired — use solar_world_create_or_import");
    }

    /** Default NO_OP implementation for loaders that don't require Solar persistence (e.g. tests). */
    SolarStore NO_OP = new SolarStore() {};
}
