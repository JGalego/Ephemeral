package io.github.jgalego.ephemeral

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.text.InputType
import android.view.Menu
import android.view.MenuItem
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.ListView
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
        menu.add(0, MENU_CREDENTIAL, 0, R.string.credential_title)
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean {
        if (item.itemId == MENU_CREDENTIAL) {
            askForCredential()
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

    private fun askForCredential() {
        val field = EditText(this).apply {
            hint = getString(R.string.credential_hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setSingleLine()
        }

        val dialog = AlertDialog.Builder(this)
            .setTitle(R.string.credential_title)
            .setMessage(R.string.credential_explains)
            .setView(field)
            .setPositiveButton(R.string.save) { _, _ ->
                val key = field.text.toString().trim()
                if (key.isNotEmpty()) {
                    Credential.save(this, key)
                    Engine.refreshCredential(this)
                    say(getString(R.string.credential_saved))
                }
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

    private fun report(failure: Throwable) {
        say(failure.message ?: getString(R.string.app_name))
    }

    private fun say(message: String?) {
        status.text = message.orEmpty()
        status.visibility = if (message == null) View.GONE else View.VISIBLE
    }

    private companion object {
        const val MENU_CREDENTIAL = 1
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
