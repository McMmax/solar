package dev.terrorify.solar;

import org.jetbrains.annotations.NotNull;

/**
 * Consumer interface for receiving encoded canonical Solar chunk blobs during world import.
 *
 * <p>Implementations typically buffer chunk blobs and commit them in batches to the
 * SpacetimeDB {@code solar_chunk_base_commit_batch} reducer.</p>
 */
@FunctionalInterface
public interface SolarChunkSink {
    /**
     * Accepts one encoded Solar chunk column.
     *
     * @param chunkX       chunk X coordinate
     * @param chunkZ       chunk Z coordinate
     * @param encodedChunk canonical Solar encoded chunk blob
     * @throws Exception if dispatch or committing the chunk to SpacetimeDB fails
     */
    void put(int chunkX, int chunkZ, byte @NotNull [] encodedChunk) throws Exception;
}

