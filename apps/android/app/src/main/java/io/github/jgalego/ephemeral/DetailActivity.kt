package io.github.jgalego.ephemeral

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Typeface
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.webkit.WebView
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Spinner
import android.widget.TextView

/** One application: what it is, what it wants, and what you have said to it. */
class DetailActivity : Activity() {
    private lateinit var application: String

    private lateinit var purpose: TextView
    private lateinit var stateLabel: TextView
    private lateinit var explanation: TextView
    private lateinit var description: TextView
    private lateinit var generate: Button
    private lateinit var status: TextView
    private lateinit var form: LinearLayout
    private lateinit var permissions: LinearLayout

    /** How to read each form control back, by the input's name. */
    private val filled = mutableMapOf<String, () -> String>()
    private lateinit var footnotes: TextView
    private lateinit var whereItRuns: TextView

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        setContentView(R.layout.activity_detail)

        application = intent.getStringExtra(APPLICATION).orEmpty()
        if (application.isEmpty()) {
            finish()
            return
        }

        purpose = findViewById(R.id.purpose)
        stateLabel = findViewById(R.id.state)
        explanation = findViewById(R.id.explanation)
        description = findViewById(R.id.description)
        generate = findViewById(R.id.generate)
        status = findViewById(R.id.status)
        form = findViewById(R.id.form)
        permissions = findViewById(R.id.permissions)
        footnotes = findViewById(R.id.footnotes)
        whereItRuns = findViewById(R.id.where_it_runs)

        generate.setOnClickListener { write() }
    }

    override fun onResume() {
        super.onResume()
        load()
    }

    private fun load() {
        Engine.submit(this, { Engine.detail(application) }) { outcome ->
            outcome.onSuccess(::draw).onFailure(::report)
        }
    }

    private fun write() {
        if (!Credential.present(this)) {
            say(getString(R.string.needs_credential))
            return
        }

        generate.isEnabled = false
        say(getString(R.string.generating))

        Engine.submit(this, { Engine.generate(application) }) { outcome ->
            generate.isEnabled = true
            outcome
                .onSuccess { page ->
                    say(null)
                    draw(page)
                }
                .onFailure(::report)
        }
    }

    private fun draw(page: Detail) {
        title = page.summary.name
        purpose.text = page.summary.purpose
        // The same pill, in the same colours, as the list this page was opened
        // from — and the colour is the lifecycle's opinion, not this screen's.
        stateLabel.text = page.summary.state
        stateLabel.setTextColor(getColor(Palette.forState(page.summary.stateKind)))
        explanation.text = page.explanation
        // Empty when it would only repeat the explanation, which the service
        // layer now decides for every client at once. A photograph of this
        // screen is what showed the same sentence twice, one line apart.
        description.text = page.description
        description.visibility = if (page.description.isBlank()) View.GONE else View.VISIBLE

        generate.setText(if (page.versions.isEmpty()) R.string.generate else R.string.generate_again)

        drawForm(page)

        permissions.removeAllViews()
        if (page.outstanding.isNotEmpty()) {
            permissions.addView(heading(getString(R.string.asked_for)))
            page.outstanding.forEach { permissions.addView(asking(it)) }
        }
        if (page.allowed.isNotEmpty()) {
            permissions.addView(heading(getString(R.string.allowed_to)))
            page.allowed.forEach { permissions.addView(held(it)) }
        }
        if (page.isolated && page.outstanding.isEmpty()) {
            permissions.addView(quiet(getString(R.string.wants_nothing)))
        }

        val notes = mutableListOf("${getString(R.string.retention)}: ${page.retention}")
        if (page.versions.isNotEmpty()) {
            notes += getString(R.string.versions) + ":\n" + page.versions.joinToString("\n")
        }
        footnotes.text = notes.joinToString("\n\n")

        // What this device can do with *this* application. A fixed sentence
        // said the same thing about every application, and was wrong about half
        // of them from the moment a phone could run one.
        whereItRuns.setText(
            when {
                page.runtimeKind == null -> R.string.phone_limits
                page.runsHere() -> R.string.runs_here
                else -> R.string.needs_a_computer
            },
        )
    }

    /**
     * Draws what the application says it takes.
     *
     * Every generated application so far has been a command-line tool with
     * flags, and a phone has no terminal to type one into. So the application
     * declares its shape and this draws it — one form renderer for every
     * application, rather than asking a model to write a screen as well as a
     * program.
     *
     * Nothing here composes an argument vector. The engine does that, from the
     * same declaration, so this and the terminal cannot disagree about what a
     * filled-in form means.
     */
    private fun drawForm(page: Detail) {
        form.removeAllViews()
        filled.clear()

        // No declaration is not an empty form. Most applications ever generated
        // declared nothing, and drawing a form with no fields would claim they
        // take nothing.
        if (page.takes.isNotEmpty()) {
            form.addView(heading(getString(R.string.what_it_takes)))

            for (field in page.takes) {
                form.addView(label(field))
                form.addView(control(field))
                field.help?.let { form.addView(quiet(it)) }
            }
        }

        // Outside that, deliberately. The button used to live inside the form,
        // so an application that took no arguments — which is most of the
        // simple ones — could not be started from a phone at all: no fields, no
        // form, no button, and nothing saying why.
        if (page.runsHere()) {
            form.addView(
                Button(this).apply {
                    text = getString(R.string.run_it)
                    setOnClickListener { runWithForm() }
                },
            )
        }
    }

    /** The field's name, marked when it cannot be left out. */
    private fun label(field: Field): View = TextView(this).apply {
        text = if (field.required) {
            getString(R.string.required_field, field.label)
        } else {
            field.label
        }
        setTextColor(getColor(R.color.ink))
        setPadding(0, dp(12), 0, dp(4))
    }

    /** The control for one field, remembered so its value can be read back. */
    private fun control(field: Field): View = describing(field, drawn(field))

    /**
     * Gives a control the name the application gave it.
     *
     * A form drawn from a declaration has no ids — the fields are whatever the
     * application said it takes, and there is no layout naming them. So a
     * screen reader announcing "edit box" for every one of six fields is the
     * default, and it is useless. The label the application wrote is the right
     * announcement, and it is also what lets the device walkthrough drive a
     * form nobody wrote by hand.
     */
    private fun describing(field: Field, control: View): View = control.apply {
        contentDescription = field.label
    }

    private fun drawn(field: Field): View = when (field.kind) {
        "flag" -> CheckBox(this).apply {
            isChecked = field.default == "true"
            // A checkbox says true or false; the engine decides that false
            // means the flag is not passed at all.
            filled[field.name] = { isChecked.toString() }
        }

        "choice" -> Spinner(this).apply {
            val options = field.options
            this.adapter = ArrayAdapter(
                this@DetailActivity,
                android.R.layout.simple_spinner_dropdown_item,
                options,
            )
            field.default
                ?.let(options::indexOf)
                ?.takeIf { it >= 0 }
                ?.let { chosen -> setSelection(chosen) }
            filled[field.name] = { options.getOrNull(selectedItemPosition).orEmpty() }
        }

        else -> EditText(this).apply {
            setSingleLine()
            if (field.kind == "number") {
                inputType = InputType.TYPE_CLASS_NUMBER
            }
            // A file field is a path the application will be handed. It is not
            // a permission: whether it can be opened is the sandbox's answer,
            // and the sandbox holds only what was granted.
            hint = when (field.kind) {
                "file" -> getString(R.string.a_file)
                "folder" -> getString(R.string.a_folder)
                else -> field.default.orEmpty()
            }
            setText(field.default.orEmpty())
            filled[field.name] = { text.toString() }
        }
    }

    /**
     * Reads the form and asks the engine what command it means.
     *
     * A refusal comes back in the domain's words — "The earlier file is needed
     * before this can run" — rather than in anything this screen invents.
     */
    private fun runWithForm() {
        val answers = filled.mapValues { (_, read) -> read() }

        Engine.submit(this, { Engine.arguments(application, answers) }) { outcome ->
            outcome
                .onSuccess(::runNow)
                .onFailure { say(it.message) }
        }
    }

    /**
     * Runs it, here, on this device.
     *
     * This screen used to show the command the form produced and explain that a
     * phone could not run it. That was true of a phone with only a container
     * runtime to reach for, and it is not true any more (ADR-0021).
     *
     * Off the drawing thread, through [Engine.submit], because a run blocks for
     * as long as the application takes and thirty seconds of a frozen screen is
     * worse than any answer is good.
     */
    private fun runNow(arguments: List<String>) {
        say(getString(R.string.running))

        Engine.submit(this, { Engine.run(application, arguments) }) { outcome ->
            outcome
                .onSuccess(::showWhatItDid)
                .onFailure { say(it.message) }
        }
    }

    /**
     * What the application did, in its own words.
     *
     * A non-zero exit is shown as a failure *and* keeps the output. A program
     * that failed has usually just explained why, and replacing that with
     * "something went wrong" throws away the only thing that says what.
     */
    private fun showWhatItDid(ran: Ran) {
        val dialog = AlertDialog.Builder(this)
            .setTitle(
                if (ran.succeeded) {
                    getString(R.string.it_ran)
                } else {
                    getString(R.string.it_failed, ran.exitCode)
                },
            )
            .setPositiveButton(R.string.close, null)

        if (ran.isPage() && ran.succeeded) {
            dialog.setView(page(ran))
        } else {
            dialog.setMessage(plainly(ran))
        }

        dialog.show()
    }

    /** What it printed, and anything it was not given, as words. */
    private fun plainly(ran: Ran): String {
        val said = StringBuilder()

        if (ran.caveats().isNotEmpty()) {
            said.append(getString(R.string.not_given, ran.caveats().joinToString("\n")))
            said.append("\n\n")
        }
        said.append(ran.output.ifBlank { getString(R.string.nothing_printed) })

        return said.toString()
    }

    /**
     * The page an application wrote, rendered.
     *
     * **The interesting part is everything this view cannot do.** A
     * WebAssembly application has no socket, so it writes a page rather than
     * serving one — and the page is then displayed by a view with scripts off,
     * file access off, network loads blocked, and no base URL. Markup a model
     * wrote is untrusted content, and it is treated as content.
     *
     * Blocking network loads is not belt and braces. *This application holds
     * `INTERNET`*, because generating needs it, and a `WebView` inherits that:
     * without `blockNetworkLoads` a generated page could put what it was shown
     * into the query string of an image and send it anywhere. The sandbox the
     * application ran in has no sockets; the view that displays its output must
     * not quietly supply one.
     *
     * That is also why showing somebody a user interface costs no network
     * permission at all. The usual arrangement — an application serving a page
     * on a port — needs one, and then the same permission lets it talk to
     * anybody.
     */
    private fun page(ran: Ran): View {
        val shown = WebView(this).apply {
            settings.javaScriptEnabled = false
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.blockNetworkLoads = true
            settings.blockNetworkImage = true
            isVerticalScrollBarEnabled = true
            // No base URL. Without one every relative reference resolves to
            // nothing, so a page cannot reach a file next to it or anywhere
            // else — which is the point.
            loadDataWithBaseURL(null, ran.output, "text/html", "utf-8", null)
        }

        if (ran.caveats().isEmpty()) {
            return shown
        }

        // What was granted and not given effect is said in Ephemeral's own
        // voice, above the page, rather than inside content the application
        // wrote — where it could be styled to look like anything at all.
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(quiet(getString(R.string.not_given, ran.caveats().joinToString("\n"))))
            addView(
                shown,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    dp(360),
                ),
            )
        }
    }

    /** A capability that has been asked for, with the two answers to it. */
    private fun asking(capability: Capability): View {
        val row = column(capability.risk)
        row.addView(strong(capability.wants))

        // The stated reason is presented as a claim, never as a fact: it was
        // written by whatever asked, and it is the one part of a request a
        // person cannot check.
        capability.reason?.takeIf(String::isNotBlank)?.let {
            row.addView(quiet(getString(R.string.it_says, it)))
        }
        // What would follow from allowing it, in the colour of what it costs.
        row.addView(consequence(capability))

        val answers = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.END
        }
        answers.addView(
            Button(this).apply {
                setText(R.string.refuse)
                setOnClickListener { answer(capability, allow = false) }
            },
        )
        answers.addView(
            Button(this).apply {
                setText(R.string.allow)
                setOnClickListener {
                    if (capability.needsExplicitConfirmation) {
                        confirm(capability)
                    } else {
                        answer(capability, allow = true)
                    }
                }
            },
        )
        row.addView(answers)
        return row
    }

    /** A capability already granted. */
    private fun held(capability: Capability): View {
        val row = column(capability.risk)
        row.addView(strong("\u2713 " + capability.wants))
        row.addView(consequence(capability))
        return row
    }

    /**
     * A second, deliberate step for the requests that deserve one.
     *
     * The engine decides which those are — a client that made its own judgement
     * would be a second opinion about what is dangerous, and two opinions is
     * how they start disagreeing.
     */
    private fun confirm(capability: Capability) {
        AlertDialog.Builder(this)
            .setTitle(capability.wants)
            .setMessage(capability.ifAllowed)
            .setPositiveButton(R.string.allow) { _, _ -> answer(capability, allow = true) }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun answer(capability: Capability, allow: Boolean) {
        Engine.submit(this, { Engine.decide(application, capability.capability, allow) }) { outcome ->
            outcome.onSuccess { load() }.onFailure(::report)
        }
    }

    private fun report(failure: Throwable) {
        say(failure.message ?: getString(R.string.app_name))
    }

    private fun say(message: String?) {
        status.text = message.orEmpty()
        status.visibility = if (message == null) View.GONE else View.VISIBLE
    }

    // --- small view helpers, so the drawing above reads as drawing ---

    /**
     * One thing an application has asked for, or one it already holds.
     *
     * A card rather than a run of text, and the same card the window draws.
     * `risk` colours the left edge and the sentence saying what would follow
     * from allowing it — never the reassurance that it can be taken back,
     * which in crimson reads as a warning and says the opposite of what it is.
     */
    private fun column(risk: String? = null) = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        background = getDrawable(R.drawable.card)
        setPadding(dp(14), dp(14), dp(14), dp(14))
        layoutParams = LinearLayout.LayoutParams(
            LinearLayout.LayoutParams.MATCH_PARENT,
            LinearLayout.LayoutParams.WRAP_CONTENT,
        ).apply { setMargins(0, dp(8), 0, 0) }

        if (risk != null) {
            // A hairline of the risk colour down the leading edge, so a
            // critical request cannot be mistaken for an ordinary one at a
            // glance — which is the only look most requests ever get.
            val stripe = android.graphics.drawable.LayerDrawable(
                arrayOf(
                    android.graphics.drawable.ColorDrawable(getColor(Palette.forRisk(risk))),
                    getDrawable(R.drawable.card),
                ),
            )
            stripe.setLayerInset(1, dp(3), 0, 0, 0)
            background = stripe
        }
    }

    private fun heading(text: String) = TextView(this).apply {
        this.text = text
        setTextColor(getColor(R.color.ink_quiet))
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 16f)
        setTypeface(typeface, Typeface.BOLD)
        setPadding(0, dp(16), 0, 0)
    }

    private fun strong(text: String) = TextView(this).apply {
        this.text = text
        setTextColor(getColor(R.color.ink))
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 15f)
    }

    /**
     * What allowing this would let the application do, in its risk's colour.
     *
     * The level is also said in words, next to it. Colour alone is not a way to
     * tell somebody that a permission is dangerous — not everybody sees the
     * difference between amber and rose, and the one who does not is the one
     * least able to say so.
     */
    private fun consequence(capability: Capability) = TextView(this).apply {
        text = capability.ifAllowed + "  (" + capability.risk + ")"
        setTextColor(getColor(Palette.forRisk(capability.risk)))
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
        setPadding(0, dp(6), 0, 0)
    }

    private fun quiet(text: String) = TextView(this).apply {
        this.text = text
        setTextColor(getColor(R.color.ink_quiet))
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
        setPadding(0, dp(4), 0, 0)
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        /** Which application to show. */
        const val APPLICATION = "application"
    }
}
