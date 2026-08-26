package dev.terrorify.solar;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Focused unit tests for direct Anvil → Solar chunk importing
 * ({@link DirectAnvilImporter} / {@link SolarImport#directAnvil}).
 *
 * <p>These tests are purely in-process and deterministic: no Docker, network,
 * SpacetimeDB, or MCPClient is required.  The {@link FakeChunkReader} and
 * {@link FakeChunkSink} fixtures provide full control over every read and write.
 */
class DirectAnvilImporterTest {

    // -------------------------------------------------------------------------
    // 1. Anvil root detection
    // -------------------------------------------------------------------------

    /** Classic layout: world/ contains region/ directly. */
    @Test
    void detectsClassicAnvilRoot(@TempDir Path tmp) {
        Path world = tmp.resolve("classicWorld");
        assertDoesNotThrow(() -> Files.createDirectories(world.resolve("region")));
        assertNotNull(SolarAnvilConverter.resolveAnvilRoot(world),
                "Classic region/ layout must be recognised as an Anvil root");
    }

    /** Modern layout: world/dimensions/minecraft/overworld/region/. */
    @Test
    void detectsModernDimensionAnvilRoot(@TempDir Path tmp) {
        Path world = tmp.resolve("modernWorld");
        assertDoesNotThrow(() ->
                Files.createDirectories(world.resolve("dimensions/minecraft/overworld/region")));
        assertNotNull(SolarAnvilConverter.resolveAnvilRoot(world),
                "Modern dimensions/minecraft/overworld/region/ layout must be recognised");
    }

    /** A directory with no region/ anywhere must resolve to null, and importWorld must throw. */
    @Test
    void rejectsNonAnvilRoot(@TempDir Path tmp) {
        Path notAnvil = tmp.resolve("notAnvil");
        assertDoesNotThrow(() -> Files.createDirectories(notAnvil));
        assertNull(SolarAnvilConverter.resolveAnvilRoot(notAnvil),
                "Directory with no region/ subtree must resolve to null");
        var ex = assertThrows(dev.terrorify.solar.exception.InvalidAnvilWorldException.class,
                () -> SolarImport.directAnvil(notAnvil, 0, 0, 0, 0,
                        (r, x, z) -> new byte[]{1}, (x, z, b) -> {}),
                "importWorld must throw InvalidAnvilWorldException for a non-Anvil root");
        assertEquals(notAnvil, ex.getWorldDirectory());
    }


    // -------------------------------------------------------------------------
    // 2. Direct chunk import iteration
    // -------------------------------------------------------------------------

    /** Every (x, z) coordinate pair in the rectangle must be visited exactly once. */
    @Test
    void iteratesEveryCoordinateInBounds(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("iterWorld");
        Files.createDirectories(world.resolve("region"));

        var visited = new ArrayList<String>();
        // Reader records every call; returns non-null so the sink is invoked too.
        DirectAnvilImporter.ChunkReader reader = (root, x, z) -> {
            visited.add(x + "," + z);
            return new byte[]{(byte) x, (byte) z};
        };

        SolarImport.directAnvil(world, -1, 1, -1, 1, reader, (x, z, b) -> {});

        // 3×3 = 9 chunks
        assertEquals(9, visited.size(), "All 9 coordinate pairs in [-1,1]×[-1,1] must be visited");
        // Spot-check corners
        assertTrue(visited.contains("-1,-1"));
        assertTrue(visited.contains("-1,1"));
        assertTrue(visited.contains("1,-1"));
        assertTrue(visited.contains("1,1"));
    }

    /** Only chunks with non-null, non-empty blobs must be forwarded to the sink. */
    @Test
    void importsOnlyReaderProvidedChunks(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("sparseWorld");
        Files.createDirectories(world.resolve("region"));

        var seen = new ArrayList<String>();
        long count = SolarImport.directAnvil(world, -1, 1, -1, 1,
                (root, x, z) -> (x == 0 && z == 0) ? new byte[]{1, 2, 3} : null,
                (x, z, bytes) -> seen.add(x + "," + z + ":" + bytes.length));

        assertEquals(1, count, "Exactly one chunk blob was provided by the reader");
        assertEquals(List.of("0,0:3"), seen,
                "Only the non-null blob must reach the sink, preserving coordinates and length");
    }

    // -------------------------------------------------------------------------
    // 3. Chunk sink invocation
    // -------------------------------------------------------------------------

    /** The sink must receive blobs preserving their content, coordinates, and byte length. */
    @Test
    void sinkReceivesBlobContentUnmodified(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("sinkWorld");
        Files.createDirectories(world.resolve("region"));

        byte[] payload = {0x53, 0x6F, 0x6C, 0x72, 0x01}; // "Solr" + version 1

        FakeChunkSink sink = new FakeChunkSink();
        SolarImport.directAnvil(world, 5, 5, 7, 7,
                (root, x, z) -> (x == 5 && z == 7) ? payload.clone() : null,
                sink);

        assertEquals(1, sink.records.size());
        FakeChunkSink.Record rec = sink.records.get(0);
        assertEquals(5, rec.chunkX());
        assertEquals(7, rec.chunkZ());
        assertArrayEquals(payload, rec.blob(),
                "Sink must receive the exact bytes the reader produced, unmodified");
    }

    // -------------------------------------------------------------------------
    // 4. Coordinate preservation
    // -------------------------------------------------------------------------

    /** Negative chunk coordinates must be passed through without sign-corruption. */
    @Test
    void preservesNegativeChunkCoordinates(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("negWorld");
        Files.createDirectories(world.resolve("region"));

        FakeChunkSink sink = new FakeChunkSink();
        SolarImport.directAnvil(world, -16, -16, -32, -32,
                (root, x, z) -> new byte[]{1},
                sink);

        assertEquals(1, sink.records.size());
        assertEquals(-16, sink.records.get(0).chunkX(), "Negative chunkX must be preserved");
        assertEquals(-32, sink.records.get(0).chunkZ(), "Negative chunkZ must be preserved");
    }

    // -------------------------------------------------------------------------
    // 5. Empty / missing chunk handling
    // -------------------------------------------------------------------------

    /** A reader that always returns null must yield zero imports and never call the sink. */
    @Test
    void handlesAllMissingChunksGracefully(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("emptyWorld");
        Files.createDirectories(world.resolve("region"));

        AtomicBoolean sinkCalled = new AtomicBoolean(false);
        long count = SolarImport.directAnvil(world, 0, 4, 0, 4,
                (root, x, z) -> null,
                (x, z, b) -> sinkCalled.set(true));

        assertEquals(0, count, "No chunks were provided; import count must be zero");
        assertFalse(sinkCalled.get(), "Sink must never be called when every chunk is absent");
    }

    /** A zero-length blob must be treated as absent (not forwarded to the sink). */
    @Test
    void treatsEmptyBlobAsAbsent(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("emptyBlobWorld");
        Files.createDirectories(world.resolve("region"));

        AtomicBoolean sinkCalled = new AtomicBoolean(false);
        long count = SolarImport.directAnvil(world, 0, 0, 0, 0,
                (root, x, z) -> new byte[0],   // empty, not null
                (x, z, b) -> sinkCalled.set(true));

        assertEquals(0, count, "Zero-length blob must not count as an import");
        assertFalse(sinkCalled.get(), "Sink must not be called for a zero-length blob");
    }

    // -------------------------------------------------------------------------
    // 6. Import failure propagation
    // -------------------------------------------------------------------------

    /** An exception thrown by the reader must propagate out of importWorld. */
    @Test
    void propagatesReaderException(@TempDir Path tmp) {
        Path world = tmp.resolve("failReaderWorld");
        assertDoesNotThrow(() -> Files.createDirectories(world.resolve("region")));

        assertThrows(IOException.class,
                () -> SolarImport.directAnvil(world, 0, 0, 0, 0,
                        (root, x, z) -> { throw new IOException("simulated MCA read failure"); },
                        (x, z, b) -> {}),
                "IOException from reader must propagate uncaught through importWorld");
    }

    /** An exception thrown by the sink must propagate out of importWorld. */
    @Test
    void propagatesSinkException(@TempDir Path tmp) {
        Path world = tmp.resolve("failSinkWorld");
        assertDoesNotThrow(() -> Files.createDirectories(world.resolve("region")));

        assertThrows(RuntimeException.class,
                () -> SolarImport.directAnvil(world, 0, 0, 0, 0,
                        (root, x, z) -> new byte[]{1},
                        (x, z, b) -> { throw new RuntimeException("simulated SpacetimeDB write failure"); }),
                "RuntimeException from sink must propagate uncaught through importWorld");
    }
}
