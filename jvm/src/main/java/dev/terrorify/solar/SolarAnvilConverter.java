package dev.terrorify.solar;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

import java.io.IOException;
import java.lang.reflect.Method;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Legacy Harbor-only Anvil-to-Polar conversion utility.
 *
 * <p><b>Deprecated for Solar runtime use</b>: The Solar runtime utilizes direct Anvil import
 * via {@link DirectAnvilImporter} (or {@link SolarImport#directAnvil}) to stream chunks directly
 * into SpacetimeDB {@code solar_chunk_base} without creating Polar intermediate artifacts.
 * This class is retained strictly for Harbor cold-storage and legacy offline tooling compatibility.</p>
 */
@Deprecated(forRemoval = false)
public final class SolarAnvilConverter {
    private SolarAnvilConverter() {}

    /**
     * Converts {@code worldDirectory} into an intermediate Polar file at {@code output}.
     *
     * <p><b>Harbor-only compatibility method.</b> Do not call from the Solar runtime.</p>
     *
     * @param worldDirectory source Anvil world root
     * @param output         destination {@code .polar} file path
     * @return destination path
     * @throws IOException if directory is missing or Polar converter fails
     */
    @Deprecated(forRemoval = false)
    public static Path convert(@NotNull Path worldDirectory, @NotNull Path output) throws IOException {
        Path anvil = resolveAnvilRoot(worldDirectory);
        if (anvil == null) throw new IOException("No Anvil region directory under " + worldDirectory);
        try {
            Class<?> anvilPolar = Class.forName("net.hollowcube.polar.AnvilPolar");
            Class<?> polarWriter = Class.forName("net.hollowcube.polar.PolarWriter");
            Method read = anvilPolar.getMethod("anvilToPolar", Path.class);
            Object world = read.invoke(null, anvil);
            Method write = java.util.Arrays.stream(polarWriter.getMethods())
                    .filter(m -> m.getName().equals("write") && m.getParameterCount() == 1)
                    .findFirst().orElseThrow(() -> new NoSuchMethodException("PolarWriter.write"));
            byte[] bytes = (byte[]) write.invoke(null, world);
            Path parent = output.toAbsolutePath().getParent();
            if (parent != null) Files.createDirectories(parent);
            Files.write(output, bytes);
            return output;
        } catch (ReflectiveOperationException e) {
            throw new IOException("Polar Anvil adapter is not present on the Stomzy runtime classpath", e);
        }
    }

    /**
     * Locate classic {@code region/} or modern {@code dimensions/minecraft/overworld/region/}
     * directory inside an Anvil world save.
     *
     * @param worldDirectory path to the Minecraft save root
     * @return path to the directory containing {@code region/}, or {@code null} if not found
     */
    public static @Nullable Path resolveAnvilRoot(@NotNull Path worldDirectory) {
        if (Files.isDirectory(worldDirectory.resolve("region"))) return worldDirectory;
        Path overworld = worldDirectory.resolve("dimensions/minecraft/overworld");
        return Files.isDirectory(overworld.resolve("region")) ? overworld : null;
    }
}

