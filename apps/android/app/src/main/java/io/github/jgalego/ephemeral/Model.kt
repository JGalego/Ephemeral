package io.github.jgalego.ephemeral

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/**
 * One service this build can be pointed at, as the engine describes it.
 *
 * Read from the engine rather than written down here. A list of providers in
 * the app is a list that is wrong the moment one is added, and this app and the
 * engine it links against do not ship on the same schedule.
 */
data class Provider(
    val name: String,
    val what: String,
    val needsCredential: Boolean,
    /** Which of [Choice]'s fields this provider reads. Show these, hide the rest. */
    val configurable: List<String>,
    val baseUrl: String?,
    val model: String?,
) {
    fun configures(field: String) = configurable.contains(field)

    companion object {
        fun from(json: JSONObject): Provider {
            val fields = json.optJSONArray("configurable") ?: JSONArray()
            return Provider(
                name = json.optString("name"),
                what = json.optString("what"),
                needsCredential = json.optBoolean("needs_credential"),
                configurable = (0 until fields.length()).map { fields.optString(it) },
                baseUrl = json.optString("base_url").ifBlank { null },
                model = json.optString("model").ifBlank { null },
            )
        }
    }
}

/**
 * One model a service says it can be asked for.
 *
 * The ceiling is carried because it is the setting most likely to be wrong and
 * the hardest to guess: a model with a 16k window refuses a request for 32k
 * outright, with a message about a field nobody typed.
 */
data class Model(val id: String, val name: String, val ceiling: Int?) {
    /** What to show in a picker: the name, and what it will hold. */
    fun label(): String = when (ceiling) {
        null -> name
        else -> "$name  ($ceiling tokens)"
    }

    companion object {
        fun from(json: JSONObject) = Model(
            id = json.optString("id"),
            name = json.optString("name").ifBlank { json.optString("id") },
            ceiling = if (json.has("ceiling")) json.optInt("ceiling") else null,
        )
    }
}

/**
 * One thing an application says it takes.
 *
 * Read from the application's own declaration rather than guessed at, and
 * rendered as a form field. A declaration is not a permission: an application
 * saying it takes a file is not one that may read any particular file, and the
 * sandbox still contains only what was granted.
 */
data class Field(
    val name: String,
    val label: String,
    val kind: String,
    val options: List<String>,
    val required: Boolean,
    val default: String?,
    val help: String?,
) {
    companion object {
        fun from(json: JSONObject): Field {
            // Both are internally tagged by the domain, so the discriminator
            // sits inside the object rather than beside it.
            val kind = json.optJSONObject("kind")?.optString("kind").orEmpty()
            val options = json.optJSONObject("kind")?.optJSONArray("options")
            return Field(
                name = json.optString("name"),
                label = json.optString("label").ifBlank { json.optString("name") },
                kind = kind,
                options = (0 until (options?.length() ?: 0)).map { options!!.optString(it) },
                required = json.optBoolean("required"),
                default = json.optString("default").ifBlank { null },
                help = json.optString("help").ifBlank { null },
            )
        }
    }
}

/**
 * What somebody chose: a service, and how it is configured.
 *
 * Deliberately not a place a credential lives. The key is sealed in the
 * keystore by [Credential]; this is ordinary preferences, and keeping the two
 * apart is what makes that true rather than aspirational.
 */
data class Choice(
    val provider: String,
    val baseUrl: String? = null,
    val model: String? = null,
) {
    /** The JSON the engine takes. Absent fields mean "that provider's default". */
    fun asJson(): String = JSONObject()
        .put("provider", provider)
        .apply {
            baseUrl?.takeIf { it.isNotBlank() }?.let { put("base_url", it) }
            model?.takeIf { it.isNotBlank() }?.let { put("model", it) }
        }
        .toString()

    companion object {
        private const val FILE = "model"
        private const val PROVIDER = "provider"
        private const val BASE_URL = "base_url"
        private const val MODEL = "model"

        /** What was chosen last, or null if nobody has chosen yet. */
        fun read(context: Context): Choice? {
            val stored = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
            val provider = stored.getString(PROVIDER, null) ?: return null
            return Choice(
                provider = provider,
                baseUrl = stored.getString(BASE_URL, null),
                model = stored.getString(MODEL, null),
            )
        }

        /** Remembers a choice across restarts. */
        fun save(context: Context, choice: Choice) {
            context.getSharedPreferences(FILE, Context.MODE_PRIVATE)
                .edit()
                .putString(PROVIDER, choice.provider)
                .putString(BASE_URL, choice.baseUrl?.takeIf { it.isNotBlank() })
                .putString(MODEL, choice.model?.takeIf { it.isNotBlank() })
                .apply()
        }
    }
}

/**
 * What one run produced.
 *
 * An application that ran and failed is not an error: it is this, with
 * [succeeded] false and the program's own words in [output]. Drawing a
 * non-zero exit as "something went wrong" and hiding the output would throw
 * away the only thing that says what.
 */
data class Ran(
    val succeeded: Boolean,
    val exitCode: Int,
    val output: String,
    /**
     * Access somebody granted that the runtime will not give effect to.
     *
     * Shown rather than swallowed. Somebody who allowed an application to read
     * a folder and watched it fail to find the folder is owed the reason, and
     * the reason is us.
     */
    val refused: List<String>,
) {
    companion object {
        fun from(json: JSONObject): Ran {
            val refused = json.optJSONArray("refused") ?: JSONArray()
            return Ran(
                succeeded = json.optBoolean("succeeded"),
                exitCode = json.optInt("exit_code"),
                output = json.optString("output"),
                refused = (0 until refused.length()).map { refused.optString(it) },
            )
        }
    }
}
