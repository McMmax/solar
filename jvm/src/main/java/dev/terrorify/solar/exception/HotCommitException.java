package dev.terrorify.solar.exception;

/**
 * Thrown when committing chunk blobs or lifecycle transitions to SpacetimeDB / Hot fails.
 */
public class HotCommitException extends SolarImportException {
    private final int statusCode;

    public HotCommitException(String message, int statusCode) {
        super(message + " (HTTP " + statusCode + ")");
        this.statusCode = statusCode;
    }

    public int getStatusCode() {
        return statusCode;
    }
}
