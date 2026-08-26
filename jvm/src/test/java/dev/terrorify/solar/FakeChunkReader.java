package dev.terrorify.solar;

import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

/**
 * Deterministic in-process {@link DirectAnvilImporter.ChunkReader} fixture for unit tests.
 *
 * <p>No Docker, network, SpacetimeDB, or MCPClient required. The fixture is pre-loaded
 * with a map of {@code (chunkX, chunkZ) → blob} pairs via {@link #put}; all other
 * coordinates return {@code null} (absent), matching the contract defined in
 * {@link DirectAnvilImporter.ChunkReader}.
 *
 * <p>Use {@link #ofSingle} to create a reader that returns one pre-canned blob and
 * {@link #ofConstant} to create a reader that returns the same blob for every coordinate.
 */
public final class FakeChunkReader implements DirectAnvilImporter.ChunkReader {

    private final Map<Long, byte[]> data = new HashMap<>();

    /** Register a blob for a specific {@code (chunkX, chunkZ)} pair. */
    public FakeChunkReader put(int chunkX, int chunkZ, byte[] blob) {
        data.put(key(chunkX, chunkZ), blob.clone());
        return this;
    }

    @Override
    public byte[] read(Path anvilWorld, int chunkX, int chunkZ) {
        byte[] blob = data.get(key(chunkX, chunkZ));
        return blob != null ? blob.clone() : null;
    }

    /** Returns a reader that serves {@code blob} only at {@code (cx, cz)}. */
    public static FakeChunkReader ofSingle(int cx, int cz, byte[] blob) {
        return new FakeChunkReader().put(cx, cz, blob);
    }

    /**
     * Returns a reader that returns a copy of {@code blob} for every coordinate requested.
     * Useful for saturation tests.
     */
    public static DirectAnvilImporter.ChunkReader ofConstant(byte[] blob) {
        return (anvilWorld, x, z) -> blob.clone();
    }

    private static long key(int x, int z) {
        return ((long) x << 32) | (z & 0xFFFFFFFFL);
    }
}
