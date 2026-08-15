package dev.terrorify.solar;

import net.minestom.server.MinecraftServer;
import net.minestom.server.instance.Chunk;
import net.minestom.server.instance.Instance;
import net.minestom.server.instance.block.Block;
import net.minestom.server.world.DimensionType;
import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;
import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Verifies the Java-side orchestration this plan added (SolarLoader + SolarChunkCodec +
 * SolarStore) round-trips correctly, standing in for a live SpacetimeDB connection with an
 * in-memory fake — the reducer/auth contract itself was verified live against a real
 * SpacetimeDB instance (Phase 0), and the wire format was verified in {@link SolarChunkCodecTest}.
 * This test is what's left: does saving through one SolarLoader and loading through a second,
 * independent one (simulating a server restart) reproduce the same block state.
 */
class SolarLoaderTest {

    /** In-memory stand-in for the real native-bridge-backed SolarStore. */
    static final class FakeSolarStore implements SolarStore {
        final Map<String, byte[]> chunks = new HashMap<>();
        int putChunkCalls = 0;

        @Override
        public byte @Nullable [] getChunk(@NotNull String worldId, int chunkX, int chunkZ) {
            return chunks.get(key(worldId, chunkX, chunkZ));
        }

        @Override
        public void putChunk(@NotNull String worldId, int chunkX, int chunkZ, byte @NotNull [] data) {
            chunks.put(key(worldId, chunkX, chunkZ), data);
            putChunkCalls++;
        }

        @Override
        public void applyBlockSet(@NotNull String worldId, int x, int y, int z, int state, @NotNull String actor) {}

        @Override
        public void prune(@NotNull String worldId, int chunkX, int chunkZ, long upToId) {}

        @Override
        public void putWorld(@NotNull SolarWorld world) {}

        private static String key(String worldId, int cx, int cz) {
            return worldId + ":" + cx + ":" + cz;
        }
    }

    @Test
    void saveThenLoadThroughASeparateLoaderReproducesBlockState() {
        MinecraftServer.init();

        final FakeSolarStore store = new FakeSolarStore();
        final String worldId = "test";

        // "Server 1": save a chunk with a distinctive, non-default block.
        final Instance instance1 = MinecraftServer.getInstanceManager()
                .createInstanceContainer(DimensionType.OVERWORLD, null);
        final SolarLoader loader1 = new SolarLoader(worldId, true, store);
        final Chunk chunk1 = instance1.getChunkSupplier().createChunk(instance1, 3, -2);
        chunk1.lockWriteLock();
        try {
            chunk1.getSection(0).blockPalette().fill(Block.STONE.stateId());
            chunk1.getSection(1).blockPalette().set(1, 2, 3, Block.DIAMOND_BLOCK.stateId());
        } finally {
            chunk1.unlockWriteLock();
        }

        loader1.saveChunk(chunk1);
        assertEquals(1, store.putChunkCalls, "saveChunk should push exactly one chunk to the store");

        // "Server 2" (a fresh instance + a fresh SolarLoader, same backing store — simulating restart).
        final Instance instance2 = MinecraftServer.getInstanceManager()
                .createInstanceContainer(DimensionType.OVERWORLD, null);
        final SolarLoader loader2 = new SolarLoader(worldId, true, store);
        final Chunk reloaded = loader2.loadChunk(instance2, 3, -2);

        assertNotNull(reloaded, "a chunk persisted by loader1 must be loadable by an independent loader2");
        assertEquals(Block.STONE.stateId(), reloaded.getSection(0).blockPalette().get(0, 0, 0));
        assertEquals(Block.DIAMOND_BLOCK.stateId(), reloaded.getSection(1).blockPalette().get(1, 2, 3));
    }

    @Test
    void loadReturnsNullWhenNoDataIsResident() {
        MinecraftServer.init();
        final FakeSolarStore store = new FakeSolarStore();
        final SolarLoader loader = new SolarLoader("test", true, store);
        final Instance instance = MinecraftServer.getInstanceManager()
                .createInstanceContainer(DimensionType.OVERWORLD, null);
        assertEquals(null, loader.loadChunk(instance, 99, 99));
    }
}
