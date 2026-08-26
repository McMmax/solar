package dev.terrorify.solar;

import java.util.ArrayList;
import java.util.List;

/**
 * Deterministic in-process {@link SolarChunkSink} fixture for unit tests.
 *
 * <p>No Docker, network, SpacetimeDB, or MCPClient required. Every {@link #put} call is
 * recorded in {@link #records} for assertion by the test.
 */
public final class FakeChunkSink implements SolarChunkSink {

    /** Immutable snapshot of one {@link #put} call. */
    public record Record(int chunkX, int chunkZ, byte[] blob) {}

    /** Ordered log of all {@link #put} invocations. */
    public final List<Record> records = new ArrayList<>();

    @Override
    public void put(int chunkX, int chunkZ, byte[] encodedChunk) {
        records.add(new Record(chunkX, chunkZ, encodedChunk.clone()));
    }
}
