package dev.terrorify.solar;

import dev.terrorify.solar.exception.InvalidAnvilWorldException;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Fixture-based integration test for direct Anvil world importing without external services.
 */
class DirectAnvilFixtureImportTest {

    @Test
    void testMultiRegionFixtureImport(@TempDir Path tmp) throws Exception {
        // Setup modern dimension fixture: dimensions/minecraft/overworld/region/
        Path overworldRegion = tmp.resolve("dimensions/minecraft/overworld/region");
        Files.createDirectories(overworldRegion);

        // Create dummy MCA region marker files
        Files.write(overworldRegion.resolve("r.0.0.mca"), new byte[]{1, 2, 3});
        Files.write(overworldRegion.resolve("r.-1.-1.mca"), new byte[]{1, 2, 3});

        // Fixture chunk store: map coordinate to fake Solar blob
        Map<String, byte[]> fixtureChunks = new HashMap<>();
        // Region (0,0) contains chunk (0,0) and (1,1)
        fixtureChunks.put("0,0", new byte[]{0x53, 0x6F, 0x6C, 0x72, 0x01, 0x00, 0x00, 0x00, 0x00});
        fixtureChunks.put("1,1", new byte[]{0x53, 0x6F, 0x6C, 0x72, 0x01, 0x00, 0x00, 0x00, 0x00});
        // Region (-1,-1) contains chunk (-16,-16)
        fixtureChunks.put("-16,-16", new byte[]{0x53, 0x6F, 0x6C, 0x72, 0x01, 0x00, 0x00, 0x00, 0x00});

        DirectAnvilImporter.ChunkReader reader = (world, cx, cz) -> fixtureChunks.get(cx + "," + cz);

        List<String> committedChunks = new ArrayList<>();
        SolarChunkSink sink = (cx, cz, blob) -> {
            assertNotNull(blob);
            assertTrue(blob.length >= 9);
            committedChunks.add(cx + "," + cz);
        };

        // Run direct import over coordinate bounds [-32, 31] x [-32, 31]
        long count = SolarImport.directAnvil(tmp, -32, 31, -32, 31, reader, sink);

        assertEquals(3, count, "Should import exactly 3 fixture chunks");
        assertEquals(3, committedChunks.size());
        assertTrue(committedChunks.contains("0,0"));
        assertTrue(committedChunks.contains("1,1"));
        assertTrue(committedChunks.contains("-16,-16"));
    }

    @Test
    void testInvalidFixturePathThrowsInvalidAnvilWorldException(@TempDir Path tmp) {
        Path emptyDir = tmp.resolve("emptyWorld");
        assertDoesNotThrow(() -> Files.createDirectories(emptyDir));

        assertThrows(InvalidAnvilWorldException.class, () ->
                SolarImport.directAnvil(emptyDir, 0, 0, 0, 0, (w, cx, cz) -> null, (cx, cz, b) -> {}));
    }
}
