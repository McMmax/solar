// Solar — JVM integration module.
//
// The sibling to Polar's :polar subproject: model + per-chunk codec + native transport +
// ChunkLoader for Solar's SpacetimeDB-backed, per-chunk-row world store. Lives in the Solar
// library (libraries/solar/jvm) and is consumed by Stomzy as the :solar subproject (source
// root wired in Stomzy/settings.gradle.kts via projectDir, same pattern as :polar/:hyperscope).
//
// Unlike :polar, this has no third-party upstream codec dependency — it reads/writes Minestom's
// own Palette/NetworkBuffer types directly (see FORMAT.md for the wire format).

plugins {
    id("minestom.java-library")
}

dependencies {
    // The Stomzy Minestom engine core (ChunkLoader / Instance / Chunk / Palette / NetworkBuffer).
    implementation(project(":"))
}
