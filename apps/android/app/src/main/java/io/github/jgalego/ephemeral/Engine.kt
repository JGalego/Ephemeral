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
    /**
     * The same state, as the kind of thing it is: working, idle, active,
     * attention. What colour a state is drawn in follows from this rather than
     * from a list of state names kept here, which is how a phone and a window
     * start disagreeing about which states are alarming.
     */
    val stateKind: String,
    val running: Boolean,
    val putAway: Boolean,
    val granted: Int,
    /**
     * The worst thing this application is allowed to do, if it is allowed
     * anything. Carried so the list can draw the difference between "reads one
     * folder" and "can reach anywhere" — which it could not, and did not.
     */
    val highestGrantedRisk: String?,
    val awaitingDecision: Int,
) {
    companion object {
        fun from(json: JSONObject) = Summary(
            id = json.optString("id"),
            name = json.optString("name"),
            purpose = json.optString("purpose"),
            state = json.optString("state"),
            stateKind = json.optString("state_kind"),
            running = json.optBoolean("running"),
            putAway = json.optBoolean("put_away"),
            granted = json.optInt("granted"),
            // Absent and empty are different. An unknown risk is drawn as
            // unknown rather than defaulted to `low`, which would paint a
            // reassuring green over an application holding everything.
            highestGrantedRisk = if (json.isNull("highest_granted_risk")) {
                null
            } else {
                json.optString("highest_granted_risk").ifBlank { null }
            },
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
    /**
     * What it takes, if it said. Empty means no form, not a form with no
     * fields — every application written before applications could declare
     * anything has an empty list.
     */
    val takes: List<Field>,
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
                takes = fields(json.optJSONObject("runtime")?.optJSONArray("inputs")),
            )
        }

        private fun fields(array: JSONArray?): List<Field> =
            (0 until (array?.length() ?: 0)).mapNotNull { index ->
                array?.optJSONObject(index)?.let(Field::from)
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

    /**
     * Every provider this build can be pointed at.
     *
     * The one native call that may happen on the calling thread: it takes no
     * session, touches no workspace, and answers from a fixed list — so the
     * single-thread rule the rest of this object exists to keep does not apply
     * to it, and making a dialog wait a round trip to open would be silly.
     */
    fun providers(): List<Provider> {
        val listed = JSONArray(Native.providers() ?: "[]")
        return (0 until listed.length()).mapNotNull { index ->
            listed.optJSONObject(index)?.let(Provider::from)
        }
    }

    /**
     * Asks the chosen service what it has — which is also whether it can be
     * reached at all.
     *
     * Unlike everything else here, this one call is meant to touch the network.
     * It is what turns "configured" into "working", and before it existed the
     * first thing that could tell the difference was a failed generation, after
     * the intent had been sent.
     */
    fun models(): List<Model> {
        val listed = JSONArray(demand(Native.models(session)))
        return (0 until listed.length()).mapNotNull { index ->
            listed.optJSONObject(index)?.let(Model::from)
        }
    }

    /**
     * Turns a filled-in form into the arguments the application receives.
     *
     * Built by the engine, never here. A phone and a terminal composing
     * argument vectors separately are two subtly different applications.
     */
    fun arguments(id: String, answers: Map<String, String>): List<String> {
        val form = JSONObject()
        for ((name, value) in answers) {
            form.put(name, value)
        }

        val built = JSONArray(demand(Native.arguments(session, id, form.toString())))
        return (0 until built.length()).map(built::getString)
    }

    /** Records a choice of service and hands it to the engine. */
    fun choose(context: Context, choice: Choice) {
        val application = context.applicationContext
        Choice.save(application, choice)
        worker.execute {
            runCatching {
                openIfNeeded(application)
                Native.setProvider(session, choice.asJson())
            }
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
            // Asked, rather than assumed. There are six ways opening can fail
            // and they used to be one sentence that named none of them — which
            // is exactly what a real phone showed, and it took reading the
            // bridge's source to narrow it down at all.
            throw EphemeralFailure(Native.lastError(0L) ?: "Ephemeral could not start, and did not say why.")
        }
        session = opened

        // Both halves of "which model", restored together. A session that came
        // back with the credential but not the choice would quietly send it to
        // whichever service is the default — which is somebody else's.
        Choice.read(context)?.let { Native.setProvider(session, it.asJson()) }
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
