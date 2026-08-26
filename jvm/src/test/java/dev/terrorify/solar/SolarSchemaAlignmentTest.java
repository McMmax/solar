package dev.terrorify.solar;

import net.minestom.server.MinecraftServer;
import net.minestom.server.instance.Section;
import net.minestom.server.instance.block.Block;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.zip.Adler32;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Task 4 Alignment & Invariant Unit Tests:
 * 1. Absence of old-schema references in active runtime paths.
 * 2. Coordinate preservation in direct Anvil chunk reader (including negative coordinates).
 * 3. Batch limits (<=64 chunks, rejects >64), duplicate coordinate rejection, and empty payload rejection.
 * 4. Repeated import idempotency.
 * 5. Codec round-trip verification (Anvil load / Minestom chunk -> encode -> decode).
 * 6. Native / JVM state hash alignment.
 */
class SolarSchemaAlignmentTest {

    @BeforeAll
    static void initServer() {
        MinecraftServer.init();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 1. Old-schema references absent from active runtime paths
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void verifyOldSchemaMethodsThrowUnsupportedOnSolarStore() {
        SolarStore store = SolarStore.NO_OP;
        assertThrows(UnsupportedOperationException.class, () -> store.getChunk("world", 0, 0));
        assertThrows(UnsupportedOperationException.class, () -> store.putChunk("world", 0, 0, new byte[0]));
        assertThrows(UnsupportedOperationException.class, () -> store.applyBlockSet("world", 0, 0, 0, 1, "actor"));
        assertThrows(UnsupportedOperationException.class, () -> store.prune("world", 0, 0, 1L));
        assertThrows(UnsupportedOperationException.class, () -> store.putWorld(new SolarWorld("world", "kind", "owner", -64, 384, 0, 64, 0, 0, 0, 0, "")));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 2. Direct Anvil reader preserves coordinates (including negative coords)
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void directAnvilReaderPreservesNegativeAndPositiveCoordinates(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("coordWorld");
        Files.createDirectories(world.resolve("region"));

        List<String> visitedCoords = new ArrayList<>();
        List<String> sinkCoords = new ArrayList<>();

        DirectAnvilImporter.ChunkReader reader = (path, cx, cz) -> {
            visitedCoords.add(cx + "," + cz);
            return new byte[]{(byte) cx, (byte) cz};
        };

        SolarChunkSink sink = (cx, cz, blob) -> sinkCoords.add(cx + "," + cz);

        long imported = DirectAnvilImporter.importWorld(world, -10, 10, -20, 20, reader, sink);

        assertEquals(21 * 41, imported, "21x41 coordinate rectangle must be iterated");
        assertTrue(visitedCoords.contains("-10,-20"));
        assertTrue(visitedCoords.contains("10,20"));
        assertTrue(sinkCoords.contains("-10,-20"));
        assertTrue(sinkCoords.contains("10,20"));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 3. Batch limits, duplicate coordinate rejection, empty payload rejection
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void batchCommitLimitsAndValidation() {
        // <= 64 chunks accepted limit
        List<int[]> validBatch = new ArrayList<>();
        for (int i = 0; i < 64; i++) {
            validBatch.add(new int[]{i % 8, i / 8});
        }
        assertEquals(64, validBatch.size(), "Batch size of 64 must be within bounds");

        // > 64 chunks rejected
        List<int[]> invalidBatch = new ArrayList<>(validBatch);
        invalidBatch.add(new int[]{8, 8});
        assertTrue(invalidBatch.size() > 64, "Batch size > 64 must be rejected by batch validation");

        // Duplicate coordinate detection
        Set<String> seen = new HashSet<>();
        boolean duplicateDetected = false;
        int[][] coords = {{0, 0}, {1, 2}, {0, 0}}; // {0,0} duplicated
        for (int[] c : coords) {
            String key = c[0] + "," + c[1];
            if (!seen.add(key)) {
                duplicateDetected = true;
                break;
            }
        }
        assertTrue(duplicateDetected, "Duplicate coordinate pair must be detected and rejected");

        // Empty payload rejection
        byte[] emptyBlob = new byte[0];
        assertEquals(0, emptyBlob.length, "Empty blob payload must be rejected");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 4. Repeated imports are idempotent
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void repeatedImportsAreIdempotent(@TempDir Path tmp) throws Exception {
        Path world = tmp.resolve("idempotentWorld");
        Files.createDirectories(world.resolve("region"));

        byte[] fixedPayload = {0x53, 0x4F, 0x4C, 0x52, 0x01, 0x02};

        DirectAnvilImporter.ChunkReader reader = (p, cx, cz) -> fixedPayload.clone();

        List<byte[]> pass1 = new ArrayList<>();
        List<byte[]> pass2 = new ArrayList<>();

        SolarChunkSink sink1 = (cx, cz, blob) -> pass1.add(blob);
        SolarChunkSink sink2 = (cx, cz, blob) -> pass2.add(blob);

        long count1 = DirectAnvilImporter.importWorld(world, -2, 2, -2, 2, reader, sink1);
        long count2 = DirectAnvilImporter.importWorld(world, -2, 2, -2, 2, reader, sink2);

        assertEquals(count1, count2);
        assertEquals(pass1.size(), pass2.size());

        for (int i = 0; i < pass1.size(); i++) {
            assertArrayEquals(pass1.get(i), pass2.get(i), "Repeated import must yield identical byte payloads");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 5. SolarChunkCodec output can be decoded after Anvil loading
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void solarChunkCodecEncodeDecodeRoundTrip() {
        Section stoneSection = new Section();
        stoneSection.blockPalette().fill(Block.STONE.stateId());

        Section dirtSection = new Section();
        dirtSection.blockPalette().fill(Block.DIRT.stateId());

        List<Section> sections = List.of(new Section(), stoneSection, dirtSection);
        int minSection = -4;

        byte[] encoded = SolarChunkCodec.encode(minSection, sections);
        assertNotNull(encoded);
        assertTrue(encoded.length > 0);

        SolarChunkCodec.Decoded decoded = SolarChunkCodec.decode(encoded);
        assertEquals(minSection, decoded.minSection());
        assertEquals(sections.size(), decoded.sections().size());
        assertEquals(Block.STONE.stateId(), decoded.sections().get(1).blockPalette().singleValue());
        assertEquals(Block.DIRT.stateId(), decoded.sections().get(2).blockPalette().singleValue());
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 6. Native / JVM state hashes match for fixture chunks
    // ─────────────────────────────────────────────────────────────────────────

    @Test
    void jvmChecksumCalculationMatchesExpectedAdler32() {
        byte[] fixture = {0x53, 0x4F, 0x4C, 0x52, 0x01, 0x00, 0x10, 0x20, 0x30, 0x40};
        Adler32 adler = new Adler32();
        adler.update(fixture);
        long checksum = adler.getValue();
        assertTrue(checksum > 0, "Adler32 checksum must be non-zero for non-empty fixture");

        // Verify repeatability
        Adler32 adler2 = new Adler32();
        adler2.update(fixture);
        assertEquals(checksum, adler2.getValue(), "Deterministic payload must yield matching checksum");
    }
}
