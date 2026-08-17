package io.github.jgalego.ephemeral

/**
 * The engine, exactly as it is on iOS and the desktop.
 *
 * Every function here crosses into `ephemeral-android`, which forwards to the C
 * ABI in `ephemeral-ffi`. Nothing is reimplemented on this side: the lifecycle,
 * the permission ledger and the audit record are the same code a laptop runs.
 *
 * A session is safe to use from one thread at a time, which is why nothing
 * outside [Engine] is allowed to call these.
 */
internal object Native {
    init {
        System.loadLibrary("ephemeral_android")
    }

    /** Opens a session under [home], calling back into [transport] for HTTPS. Returns 0 on failure. */
    external fun open(home: String, transport: Transport): Long

    /** Closes a session. 0 is allowed and does nothing. */
    external fun close(session: Long)

    /** Supplies the model credential. Returns 0 on success. */
    external fun setCredential(session: Long, apiKey: String): Int

    /** Why the last call failed, or null. */
    external fun lastError(session: Long): String?

    /** Records a new application from a sentence. Needs no credential and no network. */
    external fun create(session: Long, intent: String): String?

    /** Every application, most recently touched first, as a JSON array. */
    external fun applications(session: Long): String?

    /** One application's page, as JSON. */
    external fun application(session: Long, id: String): String?

    /** Plans and generates an application. Blocking, and it calls back into the transport. */
    external fun generate(session: Long, id: String): String?

    /** Records an answer to one thing an application asked for. Returns 0 on success. */
    external fun decide(session: Long, id: String, capability: String, allow: Boolean): Int
}
