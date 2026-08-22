//! Printing things people can read.
//!
//! Ephemeral does things on a user's behalf without being watched, so its
//! output is often the only account of what happened. That makes readability a
//! feature rather than a polish item.
//!
//! Colour is applied only when writing to a terminal. Piped output is plain, so
//! escape codes never end up in a file, a bug report or a grep.

use std::io::IsTerminal as _;
use std::sync::OnceLock;

use ephemeral_core::{
    lifecycle::{LifecycleState, StateKind},
    permission::RiskLevel,
};

fn colour_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        // Honour the two conventions people actually set, then fall back to
        // asking whether anyone is looking.
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

fn paint(code: &str, text: &str) -> String {
    if colour_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Emphasised.
pub(crate) fn bold(text: &str) -> String {
    paint("1", text)
}

/// De-emphasised: units, timestamps, things you read only if you are looking.
pub(crate) fn dim(text: &str) -> String {
    paint("2", text)
}

/// Something went well.
pub(crate) fn good(text: &str) -> String {
    paint("32", text)
}

/// Something needs attention but is not broken.
pub(crate) fn warn(text: &str) -> String {
    paint("33", text)
}

/// Something is wrong.
pub(crate) fn bad(text: &str) -> String {
    paint("31", text)
}

/// A heading.
pub(crate) fn heading(text: &str) -> String {
    paint("1;36", text)
}

/// Colours a lifecycle state by what it means for the user, so the same state
/// always looks the same wherever it appears.
pub(crate) fn state(state: LifecycleState) -> String {
    let label = state.headline();
    match state.kind() {
        StateKind::Working => paint("36", label),
        StateKind::AwaitingUser => paint("1;33", label),
        StateKind::Idle => paint("32", label),
        StateKind::Active => paint("1;32", label),
        StateKind::Attention => paint("31", label),
        StateKind::Archived | StateKind::Deleted => paint("2", label),
    }
}

/// Colours a risk level, so a dangerous permission cannot be presented as
/// routine.
pub(crate) fn risk(level: RiskLevel) -> String {
    match level {
        RiskLevel::Low => paint("32", "low"),
        RiskLevel::Medium => paint("33", "medium"),
        RiskLevel::High => paint("1;33", "HIGH"),
        RiskLevel::Critical => paint("1;31", "CRITICAL"),
    }
}

/// The same, for a risk that arrives from the shared layers as a name.
///
/// A view carries the risk as the string the core calls it, because a client
/// that mapped a level to its own vocabulary would eventually disagree with the
/// one next to it. Anything unrecognised is drawn plainly rather than as a
/// reassuring colour — the one case where guessing is worst.
pub(crate) fn risk_named(level: &str) -> String {
    match level {
        "low" => paint("32", "low"),
        "medium" => paint("33", "medium"),
        "high" => paint("1;33", "HIGH"),
        "critical" => paint("1;31", "CRITICAL"),
        other => other.to_owned(),
    }
}

/// A bullet with a status mark, used by `doctor`.
pub(crate) fn check(ok: Option<bool>, text: &str) -> String {
    let mark = match ok {
        Some(true) => good("✓"),
        Some(false) => bad("✗"),
        None => warn("!"),
    };
    format!("  {mark} {text}")
}

/// A label/value line, aligned so a column of them reads as a table.
pub(crate) fn field(label: &str, value: &str) -> String {
    format!("  {:<14} {value}", dim(label))
}

/// How long ago something happened, in words.
///
/// "3 minutes ago" is what a person wants; an ISO timestamp is what a machine
/// wants, and machines are not the audience for this output.
pub(crate) fn relative(at: ephemeral_core::Timestamp) -> String {
    let seconds = (ephemeral_core::now() - at).num_seconds();

    if seconds < 0 {
        return "in the future".to_owned();
    }

    let (count, unit) = match seconds {
        0..=44 => return "just now".to_owned(),
        45..=5399 => (seconds / 60, "minute"),
        5400..=172_799 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };

    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Piped output must be plain: escape codes in a log file or a bug report
    /// help nobody. The test suite is not a terminal, so this is what runs here.
    #[test]
    fn colour_is_not_applied_when_nobody_is_looking() {
        assert_eq!(bold("plain"), "plain");
        assert_eq!(state(LifecycleState::Running), "Running");
        assert_eq!(risk(RiskLevel::Critical), "CRITICAL");
    }

    #[test]
    fn relative_times_read_as_words() {
        let now = ephemeral_core::now();
        assert_eq!(relative(now), "just now");
        assert_eq!(
            relative(now - chrono::Duration::minutes(3)),
            "3 minutes ago"
        );
        assert_eq!(relative(now - chrono::Duration::minutes(1)), "1 minute ago");
        assert_eq!(relative(now - chrono::Duration::hours(5)), "5 hours ago");
        assert_eq!(relative(now - chrono::Duration::days(2)), "2 days ago");
    }

    #[test]
    fn a_clock_skew_does_not_produce_nonsense() {
        let later = ephemeral_core::now() + chrono::Duration::hours(1);
        assert_eq!(relative(later), "in the future");
    }
}
