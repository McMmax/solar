package dev.terrorify.solar;

/** Mirrors the {@code solar_world} table row (see {@code libraries/solar/schema}). */
public record SolarWorld(
        String worldId,
        String kind,
        String owner,
        int minY,
        int height,
        double spawnX,
        double spawnY,
        double spawnZ,
        float spawnYaw,
        float spawnPitch,
        long seed,
        String sourceTemplate
) {}
