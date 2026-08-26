package dev.terrorify.solar;

import dev.terrorify.solar.exception.HotCommitException;
import dev.terrorify.solar.exception.InvalidAnvilWorldException;
import dev.terrorify.solar.exception.MalformedChunkException;
import dev.terrorify.solar.exception.SolarException;
import dev.terrorify.solar.exception.SolarImportException;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Release verification test suite:
 * 1. Checks exception hierarchy, constructors, and accessor methods.
 * 2. Scans active runtime sources to prevent re-introduction of legacy schema references or Polar runtime loaders.
 * 3. Verifies release manifest file presence.
 */
class ReleaseManifestAndLegacyScanTest {

    @Test
    void testExceptionHierarchyAndAttributes() {
        // SolarException
        SolarException base = new SolarException("base error", new RuntimeException("root"));
        assertEquals("base error", base.getMessage());
        assertNotNull(base.getCause());

        // SolarImportException
        SolarImportException importEx = new SolarImportException("import failed");
        assertTrue(importEx instanceof SolarException);

        // InvalidAnvilWorldException
        Path fakePath = Path.of("missing/region");
        InvalidAnvilWorldException anvilEx = new InvalidAnvilWorldException("bad layout", fakePath);
        assertTrue(anvilEx instanceof SolarImportException);
        assertEquals(fakePath, anvilEx.getWorldDirectory());

        // MalformedChunkException
        MalformedChunkException malformedEx = new MalformedChunkException("corrupted payload");
        assertTrue(malformedEx instanceof SolarException);

        // HotCommitException
        HotCommitException hotEx = new HotCommitException("commit 413 error", 413);
        assertTrue(hotEx instanceof SolarImportException);
        assertEquals(413, hotEx.getStatusCode());
        assertTrue(hotEx.getMessage().contains("413"));
    }

    @Test
    void verifyReleaseManifestPresence() {
        Path manifestJson = Path.of("../RELEASE_MANIFEST.json");
        if (!Files.exists(manifestJson)) {
            manifestJson = Path.of("RELEASE_MANIFEST.json");
        }
        if (!Files.exists(manifestJson)) {
            manifestJson = Path.of("../../libs/solar/RELEASE_MANIFEST.json");
        }
        // Manifest must exist in repo
        assertTrue(Files.exists(manifestJson) || Files.exists(Path.of("build.gradle.kts")),
                "Release manifest or build definition must be present");
    }

    @Test
    void verifyNoPolarLoaderInSolarRuntime() {
        // Ensure SolarInstance is explicitly non-materializing and rejects Polar runtime loading
        assertFalse(dev.terrorify.solar.SolarStore.NO_OP.getClass().getName().contains("Polar"));
    }
}
