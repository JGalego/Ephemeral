package io.github.jgalego.ephemeral

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Typeface
import android.os.Bundle
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
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
    private lateinit var permissions: LinearLayout
    private lateinit var footnotes: TextView

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
        permissions = findViewById(R.id.permissions)
        footnotes = findViewById(R.id.footnotes)

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
        description.text = page.description

        generate.setText(if (page.versions.isEmpty()) R.string.generate else R.string.generate_again)

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
