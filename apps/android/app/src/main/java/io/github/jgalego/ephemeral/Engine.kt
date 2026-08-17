package io.github.jgalego.ephemeral

import android.content.Context
import android.os.Handler
import android.os.Looper
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/** Something the engine refused to do, in the words it used. */
class EphemeralFailure(message: String) : Exception(message)

/** One application, as it appears in a list. */
data class Summary(
    val id: String,
    val name: String,
    val purpose: String,
    val state: String,
    val running: Boolean,
    val putAway: Boolean,
    val granted: Int,
    val awaitingDecision: Int,
) {
    companion object {
        fun from(json: JSONObject) = Summary(
            id = json.optString("id"),
            name = json.optString("name"),
            purpose = json.optString("purpose"),
            state = json.optString("state"),
            running = json.optBoolean("running"),
            putAway = json.optBoolean("put_away"),
            granted = json.optInt("granted"),
            awaitingDecision = json.optInt("awaiting_decision"),
        )
    }
}

/** One capability, in the words the engine chose for it. */
data class Capability(
    val capability: String,
    val wants: String,
    val reason: String?,
    val ifAllowed: String,
    val risk: String,
    val needsExplicitConfirmation: Boolean,
) {
    companion object {
        fun from(json: JSONObject) = Capability(
            capability = json.optString("capability"),
            wants = json.optString("wants"),
            // Absent and empty are different: a request with no stated reason
            // must not be drawn as though one was given.
            reason = if (json.isNull("reason")) null else json.optString("reason"),
            ifAllowed = json.optString("if_allowed"),
            risk = json.optString("risk"),
            needsExplicitConfirmation = json.optBoolean("needs_explicit_confirmation"),
        )
    }
}

/** One application's whole page. */
data class Detail(
    val summary: Summary,
    val explanation: String,
    val description: String,
    val outstanding: List<Capability>,
    val allowed: List<Capability>,
    val isolated: Boolean,
    val versions: List<String>,
    val retention: String,
) {
    companion object {
        fun from(json: JSONObject): Detail {
            val permissions = json.optJSONObject("permissions")
            return Detail(
                summary = Summary.from(json.getJSONObject("summary")),
                explanation = json.optString("explanation"),
                description = json.optString("description"),
                outstanding = capabilities(permissions?.optJSONArray("outstanding")),
                allowed = capabilities(permissions?.optJSONArray("allowed")),
                isolated = permissions?.optBoolean("isolated") ?: true,
                versions = versions(json.optJSONArray("versions")),
                retention = json.optString("retention"),
            )
        }

        private fun capabilities(array: JSONArray?): List<Capability> =
            (0 until (array?.length() ?: 0)).mapNotNull { index ->
                array?.optJSONObject(index)?.let(Capability::from)
            }

        private fun versions(array: JSONArray?): List<String> =
            (0 until (array?.length() ?: 0)).mapNotNull { index ->
                array?.optJSONObject(index)?.let { version ->
                    "${version.optInt("sequence")} · ${version.optString("digest")} — " +
                        version.optString("reason")
                }
            }
    }
}

/**
 * The one place that talks to the engine.
 *
 * Every native call happens on a single thread, because a session is safe to
 * use from one thread at a time and that is easier to guarantee with one thread
 * than with a lock nobody remembers to take. Results are delivered back on the
 * main thread, so callers never touch a view from the wrong place.
 */
internal object Engine {
    private val worker = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "ephemeral-engine")
    }
    private val main = Handler(Looper.getMainLooper())

    private var session = 0L

    /**
     * Runs [work] against an open engine and delivers the outcome on the main
     * thread.
     *
     * Failure is passed back rather than thrown: there is no thread here for an
     * exception to usefully reach.
     */
    fun <T> submit(context: Context, work: () -> T, done: (Result<T>) -> Unit) {
        val application = context.applicationContext
        worker.execute {
            val outcome = runCatching {
                openIfNeeded(application)
                work()
            }
            main.post { done(outcome) }
        }
    }

    /** Re-reads the stored credential and hands it to the engine. */
    fun refreshCredential(context: Context) {
        val application = context.applicationContext
        worker.execute {
            runCatching {
                openIfNeeded(application)
                val key = Credential.read(application)
                if (key == null) {
                    // There is no "forget" on the C ABI: a session holds the
                    // credential in memory, so the way to drop it is to end the
                    // session. The next call opens a fresh one without it.
                    closeNow()
                } else {
                    Native.setCredential(session, key)
                }
            }
        }
    }

    fun create(intent: String): Summary = Summary.from(JSONObject(demand(Native.create(session, intent))))

    fun applications(): List<Summary> {
        val array = JSONArray(demand(Native.applications(session)))
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(Summary::from)
        }
    }

    fun detail(id: String): Detail = Detail.from(JSONObject(demand(Native.application(session, id))))

    fun generate(id: String): Detail = Detail.from(JSONObject(demand(Native.generate(session, id))))

    fun decide(id: String, capability: String, allow: Boolean) {
        if (Native.decide(session, id, capability, allow) != 0) {
            throw EphemeralFailure(lastError())
        }
    }

    private fun openIfNeeded(context: Context) {
        if (session != 0L) {
            return
        }

        val home = File(context.filesDir, "workspace")
        home.mkdirs()

        val opened = Native.open(home.absolutePath, Https)
        if (opened == 0L) {
            throw EphemeralFailure("Ephemeral could not open its files on this phone.")
        }
        session = opened

        Credential.read(context)?.let { Native.setCredential(session, it) }
    }

    private fun closeNow() {
        if (session != 0L) {
            Native.close(session)
            session = 0L
        }
    }

    private fun demand(produced: String?): String =
        produced ?: throw EphemeralFailure(lastError())

    private fun lastError(): String =
        Native.lastError(session) ?: "Ephemeral stopped without saying why."
}
