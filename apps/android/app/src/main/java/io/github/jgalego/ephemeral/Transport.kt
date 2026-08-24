package io.github.jgalego.ephemeral

import org.json.JSONArray
import java.io.BufferedReader
import java.net.HttpURLConnection
import java.net.URL
import javax.net.ssl.HttpsURLConnection

/**
 * One HTTPS round trip, performed by this app rather than by the engine.
 *
 * Ephemeral opens no sockets on a phone and brings no HTTP stack, so TLS and
 * everything around it stay with the platform. This interface is the seam; the
 * engine calls [send] whenever it needs the model.
 */
interface Transport {
    /**
     * Sends [body] to [endpoint] by [method] with exactly the headers in
     * [headersJson], returning the response body or null on any failure.
     *
     * [method] is `GET` or `POST` and is to be used as given. [body] is empty
     * for a `GET` and none is to be sent with one. The version of this that
     * always POSTed sent the models listing to `/v1/models` as a POST, which
     * OpenAI refuses with an empty body — so the connection test, the one call
     * a person makes before spending anything, failed with a JSON parse error
     * while generation against the same service worked seconds later.
     *
     * [headersJson] is `[{"name":…,"value":…}, …]` — the complete set the
     * provider composed, credential included. This app sets those and adds
     * nothing: which headers a service wants is the provider's knowledge, and
     * the version of this that wrote Anthropic's headers here is the reason a
     * phone could not be pointed at anything else.
     *
     * [status] is a one-element array to write the HTTP status into, since Java
     * has no out-parameters. Leave it zero if there is none — a generated
     * application allowed to reach a service sees this number, and an invented
     * 200 is one it might branch on.
     *
     * Called from a background thread the engine owns. It must not throw: an
     * exception here crosses back into native code, where there is nothing that
     * can act on it.
     */
    fun send(
        method: String,
        endpoint: String,
        headersJson: String,
        body: String,
        status: IntArray,
    ): String?
}

/** The platform's HTTPS stack, and nothing on top of it. */
internal object Https : Transport {
    private const val CONNECT_TIMEOUT = 30_000
    private const val READ_TIMEOUT = 180_000

    override fun send(
        method: String,
        endpoint: String,
        headersJson: String,
        body: String,
        status: IntArray,
    ): String? {
        var connection: HttpURLConnection? = null
        return try {
            // Plain HTTP would put the credential on the wire in clear text, so
            // it is refused here rather than negotiated.
            val opened = URL(endpoint).openConnection()
            if (opened !is HttpsURLConnection) return null
            connection = opened

            opened.requestMethod = method
            opened.connectTimeout = CONNECT_TIMEOUT
            opened.readTimeout = READ_TIMEOUT
            // Setting this at all makes the request a POST, whatever
            // requestMethod was told — HttpURLConnection decides the method
            // from whether there is a body to write. So it is set only when
            // there is one.
            val sends = method.equals("POST", ignoreCase = true)
            opened.doOutput = sends
            // Whatever the provider asked for, and only that.
            val headers = JSONArray(headersJson)
            for (index in 0 until headers.length()) {
                val header = headers.optJSONObject(index) ?: continue
                opened.setRequestProperty(header.optString("name"), header.optString("value"))
            }

            if (sends) {
                opened.outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }
            }

            // Read once and reported, because asking for it again is another
            // trip through the connection's own state machine.
            val code = opened.responseCode
            status[0] = code

            // The error body is returned too, not swallowed: when a provider
            // refuses, its own words are the most useful thing a person can be
            // shown, and the engine already knows how to read them.
            val stream = if (code in 200..299) opened.inputStream else opened.errorStream
            stream?.bufferedReader(Charsets.UTF_8)?.use(BufferedReader::readText)
        } catch (failure: Exception) {
            null
        } finally {
            connection?.disconnect()
        }
    }
}
