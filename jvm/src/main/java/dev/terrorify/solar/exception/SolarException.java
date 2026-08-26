package dev.terrorify.solar.exception;

/**
 * Base unchecked exception for all Solar world persistence, codec, and streaming failures.
 */
public class SolarException extends RuntimeException {
    public SolarException(String message) {
        super(message);
    }

    public SolarException(String message, Throwable cause) {
        super(message, cause);
    }
}
