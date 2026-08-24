package io.github.jgalego.ephemeral;

/**
 * One HTTPS round trip, performed by the host.
 *
 * The bridge looks this method up by name and signature at call time, so this
 * declaration is part of the contract rather than a convenience.
 */
public interface Transport {
    /**
     * Sends {@code body} to {@code endpoint} by {@code method} — "GET" or
     * "POST" — with exactly the headers in {@code headersJson}, which is
     * {@code [{"name":…,"value":…}, …]}: the complete set the provider
     * composed, credential included.
     *
     * The method is given rather than assumed. A host that always POSTed sent
     * the models listing to {@code /v1/models} as a POST, which every service
     * refuses; {@code body} is empty for a GET and none is to be sent.
     *
     * Write the HTTP status into {@code status[0]}, or leave it zero if you
     * have none. Java has no out-parameters; a generated application that has
     * been allowed to reach a service sees this number, so do not invent one.
     */
    String send(String method, String endpoint, String headersJson, String body, int[] status);
}
