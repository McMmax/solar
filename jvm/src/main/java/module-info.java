/**
 * Solar JVM integration — model, per-chunk codec, native transport, and {@code ChunkLoader}
 * for Solar's SpacetimeDB-backed, per-chunk-row world store. Consumed by Stomzy's
 * {@code :spacetime} subproject.
 */
module dev.terrorify.solar {
    requires net.minestom.server;
    requires org.slf4j;
    requires static org.jetbrains.annotations;
    requires java.base;

    exports dev.terrorify.solar;
    exports dev.terrorify.solar.exception;
}
