package dev.terrorify.solar.exception;

/**
 * Thrown when a Solar chunk byte blob fails magic, version, checksum, or payload decoding.
 */
public class MalformedChunkException extends SolarException {
    public MalformedChunkException(String message) {
        super(message);
    }

    public MalformedChunkException(String message, Throwable cause) {
        super(message, cause);
    }
}
