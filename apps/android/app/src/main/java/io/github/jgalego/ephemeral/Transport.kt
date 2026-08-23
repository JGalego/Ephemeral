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
     * POSTs [body] to [endpoint] with exactly the headers in [headersJson],
     * returning the response body or null on any failure.
     *
     * [headersJson] is `[{"name":…,"value":…}, …]` — the complete set the
     * provider composed, credential included. This app sets those and adds
     * nothing: which headers a service wants is the provider's knowledge, and
     * the version of this that wrote Anthropic's headers here is the reason a
     * phone could not be pointed at anything else.
     *
     * Called from a background thread the engine owns. It must not throw: an
     * exception here crosses back into native code, where there is nothing that
     * can act on it.
     */
    fun send(endpoint: String, headersJson: String, body: String): String?
}

/** The platform's HTTPS stack, and nothing on top of it. */
internal object Https : Transport {
    private const val CONNECT_TIMEOUT = 30_000
    private const val READ_TIMEOUT = 180_000

    override fun send(endpoint: String, headersJson: String, body: String): String? {
        var connection: HttpURLConnection? = null
        return try {
            // Plain HTTP would put the credential on the wire in clear text, so
            // it is refused here rather than negotiated.
            val opened = URL(endpoint).openConnection()
            if (opened !is HttpsURLConnection) return null
            connection = opened

            opened.requestMethod = "POST"
            opened.connectTimeout = CONNECT_TIMEOUT
            opened.readTimeout = READ_TIMEOUT
            opened.doOutput = true
            // Whatever the provider asked for, and only that.
            val headers = JSONArray(headersJson)
            for (index in 0 until headers.length()) {
                val header = headers.optJSONObject(index) ?: continue
                opened.setRequestProperty(header.optString("name"), header.optString("value"))
            }

            opened.outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }

            // The error body is returned too, not swallowed: when a provider
            // refuses, its own words are the most useful thing a person can be
            // shown, and the engine already knows how to read them.
            val stream = if (opened.responseCode in 200..299) opened.inputStream else opened.errorStream
            stream?.bufferedReader(Charsets.UTF_8)?.use(BufferedReader::readText)
        } catch (failure: Exception) {
            null
        } finally {
            connection?.disconnect()
        }
    }
}
