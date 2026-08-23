package io.github.jgalego.ephemeral;

/**
 * One HTTPS round trip, performed by the host.
 *
 * The bridge looks this method up by name and signature at call time, so this
 * declaration is part of the contract rather than a convenience.
 */
public interface Transport {
    /**
     * POSTs {@code body} to {@code endpoint} with exactly the headers in
     * {@code headersJson}, which is {@code [{"name":…,"value":…}, …]} — the
     * complete set the provider composed, credential included.
     */
    String send(String endpoint, String headersJson, String body);
}
