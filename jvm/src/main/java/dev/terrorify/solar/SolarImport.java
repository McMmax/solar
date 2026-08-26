package dev.terrorify.solar;

import dev.terrorify.solar.exception.InvalidAnvilWorldException;
import dev.terrorify.solar.exception.MalformedChunkException;
import dev.terrorify.solar.exception.SolarImportException;
import org.jetbrains.annotations.NotNull;

import java.nio.file.Path;

/**
 * Public entry point for Solar world import and chunk encoding services.
 *
 * <h2>Canonical Direct Anvil Import</h2>
 * <p>{@link #directAnvil} is the recommended, high-performance path for importing Minecraft
 * Anvil worlds into Solar persistence without creating intermediate Polar files or materializing
 * heap chunks:</p>
 *
 * <pre>{@code
 * Path anvilWorld = Path.of("path/to/saves/KitMap1");
 * DirectAnvilImporter.ChunkReader reader = (world, cx, cz) -> ...;
 * SolarChunkSink sink = (cx, cz, bytes) -> ...;
 *
 * long importedCount = SolarImport.directAnvil(
 *     anvilWorld,
 *     minChunkX, maxChunkX,
 *     minChunkZ, maxChunkZ,
 *     reader,
 *     sink
 * );
 * }</pre>
 */
public final class SolarImport {
    private SolarImport() {}

    /**
     * Legacy Harbor-only Anvil-to-Polar conversion helper.
     *
     * <p><b>Deprecated for Solar runtime use.</b> Use {@link #directAnvil} for direct Solar
     * world persistence streaming.</p>
     *
     * @param anvilWorld root directory of the Anvil world save
     * @param output     target destination path for the {@code .polar} artifact
     * @return destination file path
     * @throws java.io.IOException if directory structure is invalid or conversion fails
     */
    @Deprecated(forRemoval = false)
    public static Path anvilToPolar(@NotNull Path anvilWorld, @NotNull Path output) throws java.io.IOException {
        return SolarAnvilConverter.convert(anvilWorld, output);
    }

    /**
     * Encode a Minestom chunk using the canonical Solar wire format.
     *
     * @param chunk Minestom chunk instance
     * @return canonical Solar encoded byte blob
     */
    public static byte @NotNull [] encode(net.minestom.server.instance.@NotNull Chunk chunk) {
        return SolarChunkCodec.encode(chunk);
    }

    /**
     * Executes a direct Anvil-to-Solar import without intermediate Polar files or heap chunk allocations.
     *
     * @param anvilWorld root directory of the Minecraft world save (containing {@code region/} or modern dimensions)
     * @param minChunkX  inclusive minimum chunk X coordinate
     * @param maxChunkX  inclusive maximum chunk X coordinate
     * @param minChunkZ  inclusive minimum chunk Z coordinate
     * @param maxChunkZ  inclusive maximum chunk Z coordinate
     * @param reader     functional chunk reader
     * @param sink       destination chunk sink receiving encoded blobs
     * @return total count of non-empty chunks imported
     * @throws InvalidAnvilWorldException if the path does not contain a valid Anvil region directory
     * @throws SolarImportException       if an error occurs during reading or streaming
     * @throws Exception                  if the reader or sink throws an unhandled exception
     */
    public static long directAnvil(@NotNull Path anvilWorld, int minChunkX, int maxChunkX,
                                   int minChunkZ, int maxChunkZ,
                                   @NotNull DirectAnvilImporter.ChunkReader reader,
                                   @NotNull SolarChunkSink sink) throws Exception {
        return DirectAnvilImporter.importWorld(anvilWorld, minChunkX, maxChunkX,
                minChunkZ, maxChunkZ, reader, sink);
    }
}

