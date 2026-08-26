package dev.terrorify.solar.exception;

import java.nio.file.Path;

/**
 * Thrown when an Anvil world directory layout is invalid or missing required region folders.
 */
public class InvalidAnvilWorldException extends SolarImportException {
    private final Path worldDirectory;

    public InvalidAnvilWorldException(String message, Path worldDirectory) {
        super(message);
        this.worldDirectory = worldDirectory;
    }

    public Path getWorldDirectory() {
        return worldDirectory;
    }
}
