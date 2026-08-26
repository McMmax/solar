package dev.terrorify.solar.exception;

/**
 * Base exception for Solar world import failures and reader/sink lifecycle errors.
 */
public class SolarImportException extends SolarException {
    public SolarImportException(String message) {
        super(message);
    }

    public SolarImportException(String message, Throwable cause) {
        super(message, cause);
    }
}
