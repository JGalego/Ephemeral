package io.github.jgalego.ephemeral;

/**
 * One HTTPS round trip, performed by the host.
 *
 * The bridge looks this method up by name and signature at call time, so this
 * declaration is part of the contract rather than a convenience.
 */
public interface Transport {
    String send(String endpoint, String apiKey, String body);
}
