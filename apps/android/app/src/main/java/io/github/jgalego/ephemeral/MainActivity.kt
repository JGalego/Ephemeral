package io.github.jgalego.ephemeral

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.text.InputType
import android.text.method.PasswordTransformationMethod
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.TextView

/** Everything recorded so far, and the box you describe the next thing in. */
class MainActivity : Activity() {
    private lateinit var applications: ListView
    private lateinit var nothingYet: TextView
    private lateinit var status: TextView
    private lateinit var request: EditText
    private lateinit var create: Button
    private lateinit var adapter: Applications

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        setContentView(R.layout.activity_main)

        applications = findViewById(R.id.applications)
        nothingYet = findViewById(R.id.nothing_yet)
        status = findViewById(R.id.status)
        request = findViewById(R.id.intent)
        create = findViewById(R.id.create)

        adapter = Applications(this)
        applications.adapter = adapter
        applications.setOnItemClickListener { _, _, position, _ ->
            adapter.getItem(position)?.let(::open)
        }

        create.setOnClickListener { record() }
    }

    override fun onResume() {
        super.onResume()
        refresh()
    }

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menu.add(0, R.id.credential_menu, 0, R.string.model_title)
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        if (item.itemId == R.id.credential_menu) {
            askForModel()
            return true
        }
        return super.onOptionsItemSelected(item)
    }

    private fun refresh() {
        Engine.submit(this, Engine::applications) { outcome ->
            outcome
                .onSuccess { loaded ->
                    adapter.clear()
                    // Archived and deleted applications are not shown, but the
                    // engine still knows about them: this is a display choice,
                    // not a deletion.
                    adapter.addAll(loaded.filterNot(Summary::putAway))
                    adapter.notifyDataSetChanged()
                    nothingYet.visibility = if (adapter.count == 0) View.VISIBLE else View.GONE
                }
                .onFailure(::report)
        }
    }

    private fun record() {
        val wanted = request.text.toString().trim()
        if (wanted.isEmpty()) {
            return
        }

        create.isEnabled = false
        Engine.submit(this, { Engine.create(wanted) }) { outcome ->
            create.isEnabled = true
            outcome
                .onSuccess { recorded ->
                    request.setText("")
                    say(null)
                    open(recorded)
                }
                .onFailure(::report)
        }
    }

    private fun open(application: Summary) {
        startActivity(
            Intent(this, DetailActivity::class.java)
                .putExtra(DetailActivity.APPLICATION, application.id),
        )
    }

    /**
     * Which service generates, the key to reach it with, and a button that
     * says whether any of it works.
     *
     * One dialog rather than three, because they are one decision: a key
     * belongs to a particular service, and a model name only means something
     * once a service is chosen.
     *
     * The list of services comes from the engine. Writing it here would put a
     * list of providers in an app that ships on a different schedule from the
     * engine it links against, and it would be wrong the first time one was
     * added.
     */
    private fun askForModel() {
        val providers = Engine.providers()
        if (providers.isEmpty()) {
            say(getString(R.string.model_none))
            return
        }

        val chosen = Choice.read(this)
        var selected = providers.indexOfFirst { it.name == chosen?.provider }.coerceAtLeast(0)

        val explains = TextView(this).apply {
            setTextColor(getColor(R.color.ink_quiet))
        }

        // Radio buttons rather than a spinner, and each with a name of its own.
        //
        // Not a style preference. `--robo-directives` can click a control by
        // resource name and cannot operate a spinner at all, so with a spinner
        // here every automated run on a real phone could only ever exercise
        // whichever provider happened to be the default. A provider with no id
        // in `ids.xml` still appears and can still be chosen by hand; it just
        // cannot be named in a directive.
        val services = RadioGroup(this).apply {
            orientation = RadioGroup.VERTICAL
            providers.forEachIndexed { index, provider ->
                addView(
                    RadioButton(this@MainActivity).apply {
                        id = idFor(provider.name) ?: View.generateViewId()
                        text = provider.name
                        isChecked = index == selected
                    },
                )
            }
        }

        val baseUrl = EditText(this).apply {
            id = R.id.base_url
            setSingleLine()
            setText(chosen?.baseUrl.orEmpty())
        }

        val model = EditText(this).apply {
            id = R.id.model
            setSingleLine()
            setText(chosen?.model.orEmpty())
        }

        // The order matters, and getting it wrong is invisible until somebody
        // photographs it. `setSingleLine()` calls `setInputType` internally, so
        // calling it *after* the password variation drops the variation and the
        // key renders in the clear — which is how a rack of phones in somebody
        // else's building came to take a picture of one.
        //
        // The transformation is also set outright rather than left to follow
        // from the input type. Belt and braces on the one field where the
        // failure is a credential on a screen.
        val key = EditText(this).apply {
            id = R.id.credential
            hint = getString(R.string.credential_hint)
            setSingleLine()
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            transformationMethod = PasswordTransformationMethod.getInstance()
        }

        val said = TextView(this).apply {
            visibility = View.GONE
        }

        // The connection test. It is the only control here that can tell
        // somebody the difference between "configured" and "working", and
        // before it existed the first thing that could was a failed generation
        // — after the intent had been sent and a bill had started.
        val check = Button(this).apply {
            id = R.id.check
            text = getString(R.string.model_check)
        }

        fun current() = Choice(
            provider = providers[selected].name,
            baseUrl = baseUrl.text.toString().trim(),
            model = model.text.toString().trim(),
        )

        // Only what the chosen service actually reads. A model box on the mock
        // is a box that does nothing, and a person who fills one in and sees no
        // difference has learnt that this screen lies.
        fun showWhatApplies() {
            val provider = providers[selected]
            explains.text = provider.what
            baseUrl.visibility = if (provider.configures("base_url")) View.VISIBLE else View.GONE
            model.visibility = if (provider.configures("model")) View.VISIBLE else View.GONE
            key.visibility = if (provider.needsCredential) View.VISIBLE else View.GONE
            baseUrl.hint = provider.baseUrl ?: getString(R.string.model_base_url)
            model.hint = provider.model ?: getString(R.string.model_model)
        }
        showWhatApplies()

        services.setOnCheckedChangeListener { group, checked ->
            selected = group.indexOfChild(group.findViewById(checked)).coerceAtLeast(0)
            showWhatApplies()
        }

        check.setOnClickListener {
            said.visibility = View.VISIBLE
            said.text = getString(R.string.model_checking)
            said.setTextColor(getColor(R.color.ink_quiet))

            // Saved first, and the key with it: the check has to test what
            // generation would do, which means the settings that are about to
            // be used rather than the ones the engine still holds.
            val typed = key.text.toString().trim()
            if (typed.isNotEmpty()) {
                Credential.save(this, typed)
            }
            Engine.choose(this, current())
            if (typed.isNotEmpty()) {
                Engine.refreshCredential(this)
            }

            Engine.submit(this, Engine::models) { outcome ->
                outcome
                    .onSuccess { models ->
                        // Three states, not two. A service that answered and
                        // listed nothing is neither working nor broken, and
                        // folding it into either is what let a rejected key
                        // read as green.
                        if (models.isEmpty()) {
                            said.setTextColor(getColor(R.color.medium))
                            said.text = getString(R.string.model_none_listed)
                            return@onSuccess
                        }

                        said.setTextColor(getColor(R.color.low))
                        said.text = resources.getQuantityString(
                            R.plurals.models_reached,
                            models.size,
                            models.size,
                        )
                        // What it can be asked for, now that it has been asked.
                        // A name typed from memory is the second most common
                        // way to be almost-configured; this is the list that
                        // makes typing unnecessary.
                        offerModels(model, models)
                    }
                    // The service's own words. Ephemeral paraphrasing "invalid
                    // api key" would be a worse sentence about a problem it
                    // cannot diagnose.
                    .onFailure { failure ->
                        said.setTextColor(getColor(R.color.critical))
                        said.text = failure.message ?: getString(R.string.model_unreachable)
                    }
            }
        }

        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val margin = (16 * resources.displayMetrics.density).toInt()
            setPadding(margin, margin, margin, 0)
            addView(services)
            addView(explains)
            addView(baseUrl)
            addView(model)
            addView(key)
            addView(check)
            addView(said)
        }

        val dialog = AlertDialog.Builder(this)
            .setTitle(R.string.model_title)
            .setMessage(R.string.credential_explains)
            .setView(ScrollView(this).apply { addView(layout) })
            .setPositiveButton(R.string.save) { _, _ ->
                Engine.choose(this, current())

                val typed = key.text.toString().trim()
                if (typed.isNotEmpty()) {
                    Credential.save(this, typed)
                    Engine.refreshCredential(this)
                }
                say(getString(R.string.model_saved, providers[selected].name))
            }
            .setNegativeButton(R.string.cancel, null)

        if (Credential.present(this)) {
            dialog.setNeutralButton(R.string.credential_forget) { _, _ ->
                Credential.forget(this)
                Engine.refreshCredential(this)
                say(getString(R.string.credential_forgotten))
            }
        }

        dialog.show()
    }

    /**
     * Lets somebody pick a model from what the service actually has.
     *
     * Offered rather than imposed: the box stays typeable, because a service
     * may know models it does not list, and a picker that refused an unlisted
     * name would be a picker that decides what you may run.
     */
    private fun offerModels(field: EditText, models: List<Model>) {
        if (models.isEmpty()) {
            return
        }

        field.setOnClickListener {
            AlertDialog.Builder(this)
                .setTitle(R.string.model_pick)
                .setItems(models.map(Model::label).toTypedArray()) { _, which ->
                    field.setText(models[which].id)
                }
                .show()
        }
        field.hint = getString(R.string.model_tap_to_pick)
    }

    /**
     * The stable id for a provider, when this build has one.
     *
     * Only exists so an automated run can name a control. A provider the engine
     * has and this list does not is still shown and still choosable — it simply
     * gets a generated id, which no directive can address.
     */
    private fun idFor(provider: String): Int? = when (provider) {
        "mock" -> R.id.provider_mock
        "anthropic" -> R.id.provider_anthropic
        "openai" -> R.id.provider_openai
        else -> null
    }

    private fun report(failure: Throwable) {
        say(failure.message ?: getString(R.string.app_name))
    }

    private fun say(message: String?) {
        status.text = message.orEmpty()
        status.visibility = if (message == null) View.GONE else View.VISIBLE
    }

}

/**
 * One card per application: what was asked for, where it is, and what it holds.
 *
 * The purpose comes first because that is what a person recognises — an
 * application's generated name is Ephemeral's convenience, not theirs.
 *
 * These were three facts joined by dots in one grey subtitle, which meant an
 * application running with permission to reach the whole internet was drawn
 * exactly like an idle one that can see nothing of yours. They are three things
 * now, and the two that carry risk carry colour: the same colours the desktop
 * window uses, from the same generated palette.
 */
private class Applications(context: Context) : ArrayAdapter<Summary>(
    context,
    R.layout.row_application,
    // Not optional, and not obvious. Without the third argument `ArrayAdapter`
    // takes the layout itself to *be* a TextView and calls `setText` on it, so
    // a card made of a LinearLayout throws "ArrayAdapter requires the resource
    // ID to be a TextView" the first time `getView` runs.
    //
    // Which is to say: the list crashed the moment it had one application in
    // it. The application compiled, CI was green, and every screenshot until
    // this one had been of an empty list — a photograph of the phone with one
    // application recorded is what found it.
    R.id.purpose,
) {
    override fun getView(position: Int, recycled: View?, parent: ViewGroup): View {
        val view = super.getView(position, recycled, parent)
        val application = getItem(position) ?: return view

        view.findViewById<TextView>(R.id.purpose).text = application.purpose
        view.findViewById<TextView>(R.id.name).text = application.name

        val state = view.findViewById<TextView>(R.id.state)
        state.text = application.state
        state.setTextColor(context.getColor(Palette.forState(application.stateKind)))

        // The only thing in the list that shouts, because it is the only thing
        // that cannot proceed without a person.
        val awaiting = view.findViewById<TextView>(R.id.awaiting)
        if (application.awaitingDecision > 0) {
            awaiting.text = context.resources.getQuantityString(
                R.plurals.awaiting,
                application.awaitingDecision,
                application.awaitingDecision,
            )
            awaiting.visibility = View.VISIBLE
        } else {
            awaiting.visibility = View.GONE
        }

        val granted = view.findViewById<TextView>(R.id.granted)
        if (application.granted > 0) {
            granted.text = context.resources.getQuantityString(
                R.plurals.granted,
                application.granted,
                application.granted,
            )
            granted.setTextColor(context.getColor(Palette.forRisk(application.highestGrantedRisk)))
            granted.visibility = View.VISIBLE
        } else {
            granted.visibility = View.GONE
        }

        return view
    }
}
