package dev.terrorify.solar;

import net.minestom.server.instance.Chunk;
import net.minestom.server.instance.ChunkLoader;
import net.minestom.server.instance.Instance;
import net.minestom.server.instance.Section;
import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.util.Collection;
import java.util.List;

/**
 * {@link ChunkLoader} backed by Solar's SpacetimeDB per-chunk store (see
 * {@code libraries/solar/docs/ARCHITECTURE.md}). Pure codec + {@code ChunkLoader} plumbing —
 * all actual SpacetimeDB I/O is delegated to the injected {@link SolarStore} (owned by
 * {@code :spacetime}, which holds the one native connection).
 *
 * <p>Constructing a {@code SolarLoader} does not, by itself, point any real world at Solar —
 * a caller must instantiate one for a specific {@link Instance}, same as any other
 * {@link ChunkLoader} (see {@code libraries/solar/docs/ROADMAP.md} S2: this makes Stomzy
 * <em>capable</em> of running a Solar-backed world, it does not make any world one).
 */
public final class SolarLoader implements ChunkLoader {
    private static final Logger LOGGER = LoggerFactory.getLogger(SolarLoader.class);

    private final String worldId;
    private final boolean editable;
    private final SolarStore store;

    public SolarLoader(@NotNull String worldId, boolean editable, @NotNull SolarStore store) {
        this.worldId = worldId;
        this.editable = editable;
        this.store = store;
    }

    public @NotNull String worldId() {
        return worldId;
    }

    /** Read-only loaders (e.g. a shared practice template) never write back to Solar. */
    public boolean editable() {
        return editable;
    }

    @Override
    public @Nullable Chunk loadChunk(@NotNull Instance instance, int chunkX, int chunkZ) {
        final byte[] data = store.getChunk(worldId, chunkX, chunkZ);
        if (data == null) return null; // absent → caller's Generator fills it in.

        final SolarChunkCodec.Decoded decoded;
        try {
            decoded = SolarChunkCodec.decode(data);
        } catch (RuntimeException e) {
            LOGGER.warn("solar decode failed world={} chunk=({},{})", worldId, chunkX, chunkZ, e);
            return null;
        }

        final Chunk chunk = instance.getChunkSupplier().createChunk(instance, chunkX, chunkZ);
        chunk.lockWriteLock();
        try {
            final List<Section> decodedSections = decoded.sections();
            for (int sectionY = chunk.getMinSection(); sectionY < chunk.getMaxSection(); sectionY++) {
                final int localIndex = sectionY - decoded.minSection();
                if (localIndex < 0 || localIndex >= decodedSections.size()) {
                    LOGGER.warn("solar decode: chunk section {} out of decoded range world={} chunk=({},{})",
                            sectionY, worldId, chunkX, chunkZ);
                    continue;
                }
                final Section target = chunk.getSection(sectionY);
                final Section source = decodedSections.get(localIndex);
                target.blockPalette().copyFrom(source.blockPalette());
                target.biomePalette().copyFrom(source.biomePalette());
            }
        } finally {
            chunk.unlockWriteLock();
        }
        return chunk;
    }

    @Override
    public void saveChunk(@NotNull Chunk chunk) {
        if (!editable) return;
        final byte[] data = SolarChunkCodec.encode(chunk);
        store.putChunk(worldId, chunk.getChunkX(), chunk.getChunkZ(), data);
    }

    @Override
    public void saveChunks(@NotNull Collection<Chunk> chunks) {
        if (!editable) return;
        for (Chunk chunk : chunks) saveChunk(chunk);
    }

    @Override
    public boolean supportsParallelLoading() {
        return true;
    }

    @Override
    public boolean supportsParallelSaving() {
        return true;
    }
}
