package dev.terrorify.solar;

import dev.terrorify.solar.exception.InvalidAnvilWorldException;
import dev.terrorify.solar.exception.SolarImportException;
import org.jetbrains.annotations.NotNull;

import java.nio.file.Path;

/**
 * Direct Anvil-to-Solar world import engine.
 *
 * <p>Streams Minecraft Anvil MCA region files directly into canonical Solar chunk blobs
 * committed to SpacetimeDB via {@link SolarChunkSink}. No intermediate Polar files are
 * created and no chunk heap structures are materialized on the JVM.</p>
 *
 * <h2>Usage Example</h2>
 * <pre>{@code
 * Path anvilPath = Path.of("path/to/world");
 * DirectAnvilImporter.ChunkReader reader = (root, cx, cz) -> {
 *     // Read MCA chunk from anvilPath and return SolarChunkCodec.encode(chunk)
 *     return chunkBlob;
 * };
 * SolarChunkSink sink = (cx, cz, blob) -> {
 *     // Commit blob to SpacetimeDB solar_chunk_base reducer
 * };
 * long imported = DirectAnvilImporter.importWorld(anvilPath, -32, 31, -32, 31, reader, sink);
 * }</pre>
 */
public final class DirectAnvilImporter {
    private DirectAnvilImporter() {}

    /**
     * Functional reader interface for loading a single chunk column from an Anvil world directory.
     */
    @FunctionalInterface
    public interface ChunkReader {
        /**
         * Read one chunk and return its canonical Solar encoded byte blob, or {@code null} if empty/absent.
         *
         * @param anvilWorld root directory of the Minecraft world save
         * @param chunkX     chunk X coordinate
         * @param chunkZ     chunk Z coordinate
         * @return canonical Solar encoded chunk blob, or {@code null} / empty byte array if absent
         * @throws Exception if reading or deserializing the MCA region file fails
         */
        byte[] read(@NotNull Path anvilWorld, int chunkX, int chunkZ) throws Exception;
    }

    /**
     * Iterates all chunk coordinate pairs in {@code [minChunkX..maxChunkX] x [minChunkZ..maxChunkZ]},
     * reading valid chunks via {@code reader} and forwarding them to {@code sink}.
     *
     * @param anvilWorld root directory of the Minecraft world save
     * @param minChunkX  minimum chunk X coordinate (inclusive)
     * @param maxChunkX  maximum chunk X coordinate (inclusive)
     * @param minChunkZ  minimum chunk Z coordinate (inclusive)
     * @param maxChunkZ  maximum chunk Z coordinate (inclusive)
     * @param reader     functional chunk reader
     * @param sink       destination chunk sink
     * @return total count of non-empty chunks imported
     * @throws InvalidAnvilWorldException if the provided path does not contain a valid Anvil region directory
     * @throws SolarImportException       if an error occurs during reading or sink streaming
     * @throws Exception                  if the reader or sink throws an unhandled exception
     */
    public static long importWorld(@NotNull Path anvilWorld, int minChunkX, int maxChunkX,
                                   int minChunkZ, int maxChunkZ,
                                   @NotNull ChunkReader reader, @NotNull SolarChunkSink sink) throws Exception {
        if (SolarAnvilConverter.resolveAnvilRoot(anvilWorld) == null) {
            throw new InvalidAnvilWorldException("No Anvil region directory found under " + anvilWorld, anvilWorld);
        }
        long imported = 0;
        for (int x = minChunkX; x <= maxChunkX; x++) {
            for (int z = minChunkZ; z <= maxChunkZ; z++) {
                byte[] blob = reader.read(anvilWorld, x, z);
                if (blob != null && blob.length != 0) {
                    sink.put(x, z, blob);
                    imported++;
                }
            }
        }
        return imported;
    }
}

