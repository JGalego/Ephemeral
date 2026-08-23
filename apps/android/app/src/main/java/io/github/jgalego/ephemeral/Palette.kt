package io.github.jgalego.ephemeral

/**
 * Which colour a fact is drawn in.
 *
 * The colours themselves are in `res/values/colors.xml`, generated from
 * `crates/ephemeral-design` and shared with the desktop window. What lives here
 * is only the mapping from a word the engine used to one of them — and it is
 * one mapping, in one place, so a state cannot be alarming on a list and calm
 * on a page.
 *
 * Both functions take the engine's own vocabulary. Neither invents a fallback
 * that reassures: an unrecognised risk is drawn as ordinary text rather than as
 * green, because the one case where guessing is worst is guessing "low" about a
 * permission that turns out to be the widest one Ephemeral offers.
 *
 * The return values are colour resource ids. There is no `@ColorRes` on them
 * because that annotation lives in AndroidX, and this app deliberately has no
 * dependencies at all — the engine is the only thing it needs.
 */
internal object Palette {

    /** The colour for a lifecycle state, by the kind of state it is. */
    fun forState(kind: String): Int = when (kind) {
        // Ephemeral is doing something; it will move on by itself.
        "working" -> R.color.accent
        // It cannot continue until a person decides something.
        "awaitinguser" -> R.color.medium
        // Built, and running nothing.
        "idle" -> R.color.ink_quiet
        // Running right now.
        "active" -> R.color.low
        // Something went wrong, or was stopped.
        "attention" -> R.color.high
        // Put away, or ended.
        "archived", "deleted" -> R.color.ink_faint
        else -> R.color.ink_quiet
    }

    /** The colour for a risk level, by the name the engine gave it. */
    fun forRisk(level: String?): Int = when (level) {
        "low" -> R.color.low
        "medium" -> R.color.medium
        "high" -> R.color.high
        "critical" -> R.color.critical
        else -> R.color.ink
    }
}
