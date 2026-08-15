//! Keeping secrets out of the record.
//!
//! A log is exactly where secrets leak. Well-meaning code writes an environment
//! map into a diagnostic, and the value is then in a file that — in the audit
//! log's case — is *never modified again*.
//!
//! So redaction happens **on the write path**, before an entry is constructed.
//! A display-time filter is not a control: it fails the moment anything else
//! reads the file, copies it into a bug report, or ships it to support.
//!
//! ## What this is and is not
//!
//! This is defence in depth, not the primary control. The primary control is
//! structural: secret *values* never enter the domain model in the first place
//! — manifests record the names of settings, and the runtime injects values it
//! never hands back ([`crate::manifest`]). The redactor exists for the paths
//! that structure cannot cover: a stack trace, a container log line, a command
//! the agent chose to run.
//!
//! Pattern matching will always miss something a determined leak could contain.
//! Registering the actual secret values with [`Redactor::register_secret`] is
//! the reliable half; the patterns catch what nobody thought to register.

use std::collections::BTreeSet;

/// What replaces a redacted value.
pub const REDACTED: &str = "[redacted]";

/// The shortest secret worth registering.
///
/// Registering a very short string would turn every incidental occurrence of it
/// into `[redacted]`, which destroys the readability of the log without
/// protecting anything worth protecting.
pub const MIN_SECRET_LENGTH: usize = 8;

/// Prefixes that identify a credential regardless of what surrounds it.
///
/// Deliberately conservative: each of these is a documented, vendor-specific
/// credential prefix, so a false positive is unlikely and a match is near
/// certain to be a secret.
const CREDENTIAL_PREFIXES: &[&str] = &[
    "sk-",     // OpenAI-style API key
    "sk-ant-", // Anthropic API key
    "ghp_",    // GitHub personal access token
    "gho_",    // GitHub OAuth token
    "ghs_",    // GitHub server token
    "github_pat_",
    "glpat-",    // GitLab personal access token
    "xoxb-",     // Slack bot token
    "xoxp-",     // Slack user token
    "AKIA",      // AWS access key id
    "ASIA",      // AWS temporary access key id
    "AIza",      // Google API key
    "ya29.",     // Google OAuth token
    "hf_",       // Hugging Face token
    "dckr_pat_", // Docker Hub token
];

/// Key names whose value is a secret whatever it looks like.
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_key",
    "private_key",
    "credential",
    "authorization",
    "auth",
    "session",
];

/// Removes secrets from text before it is recorded.
///
/// Cheap to clone and safe to share. It holds the secrets it must recognise, and
/// nothing here ever prints them — including its own [`std::fmt::Debug`], which
/// reports a count rather than contents so that a panic message or a `dbg!`
/// cannot undo the thing this type exists for.
#[derive(Clone, Default)]
pub struct Redactor {
    secrets: BTreeSet<String>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("secret_count", &self.secrets.len())
            .finish()
    }
}

impl Redactor {
    /// A redactor that knows no specific secrets, and relies on patterns alone.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a value that must never appear in the record.
    ///
    /// Returns whether it was registered. Values shorter than
    /// [`MIN_SECRET_LENGTH`] are ignored: redacting a short string would mangle
    /// unrelated text without protecting anything meaningful.
    pub fn register_secret(&mut self, secret: impl Into<String>) -> bool {
        let secret = secret.into();
        if secret.trim().len() < MIN_SECRET_LENGTH {
            return false;
        }
        self.secrets.insert(secret);
        true
    }

    /// How many specific secrets this redactor recognises.
    ///
    /// The count, never the values.
    #[must_use]
    pub fn secret_count(&self) -> usize {
        self.secrets.len()
    }

    /// Returns `text` with every recognised secret replaced.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let mut out = self.redact_known_secrets(text);
        out = redact_sensitive_assignments(&out);
        redact_credential_prefixes(&out)
    }

    /// Redacts in place, avoiding an allocation when nothing matched.
    pub fn redact_in_place(&self, text: &mut String) {
        let redacted = self.redact(text);
        if redacted != *text {
            *text = redacted;
        }
    }

    /// Whether `text` still contains a recognised secret.
    ///
    /// Used by tests and by the audit log's own self-check.
    #[must_use]
    pub fn is_clean(&self, text: &str) -> bool {
        self.redact(text) == text
    }

    /// Replaces registered secrets, longest first so that a longer secret
    /// containing a shorter one is not left partially redacted.
    fn redact_known_secrets(&self, text: &str) -> String {
        let mut ordered: Vec<&String> = self.secrets.iter().collect();
        ordered.sort_by_key(|secret| std::cmp::Reverse(secret.len()));

        let mut out = text.to_owned();
        for secret in ordered {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        out
    }
}

/// Redacts `key=value` and `key: value` where the key names something secret.
fn redact_sensitive_assignments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_assignments_in_line(line));
    }
    out
}

fn redact_assignments_in_line(line: &str) -> String {
    // Work token by token so that a sensitive assignment inside a longer line
    // is redacted without destroying the rest of the line.
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    while !rest.is_empty() {
        let Some(separator) = rest.find(['=', ':']) else {
            out.push_str(rest);
            break;
        };

        let (before, after) = rest.split_at(separator);
        let separator_char = &after[..1];
        let value_start = &after[1..];

        let key = key_before_separator(before);

        out.push_str(before);
        out.push_str(separator_char);

        if SENSITIVE_KEYS
            .iter()
            .any(|sensitive| key.contains(sensitive))
        {
            // Consume the value: everything up to the next separator-ish
            // boundary.
            let leading_space: String = value_start
                .chars()
                .take_while(|c| *c == ' ' || *c == '"' || *c == '\'')
                .collect();
            let value = &value_start[leading_space.len()..];
            let end = value
                .find(|c: char| c.is_whitespace() || c == ',' || c == '}' || c == '"')
                .unwrap_or(value.len());

            if end > 0 {
                out.push_str(&leading_space);
                out.push_str(REDACTED);
                rest = &value[end..];
                continue;
            }
        }

        rest = value_start;
    }

    out
}

/// Extracts the key immediately preceding a `=` or `:`.
///
/// Reads backwards past any quoting or whitespace, then takes the trailing run
/// of identifier characters, so that `DB_PASSWORD`, `"token"` and
/// `Authorization` are all recognised as the key they are.
fn key_before_separator(before: &str) -> String {
    let trimmed = before.trim_end_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');

    trimmed
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Redacts anything that begins with a known credential prefix.
fn redact_credential_prefixes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut current = String::new();

    let flush = |current: &mut String, out: &mut String| {
        if current.is_empty() {
            return;
        }
        let looks_like_credential = CREDENTIAL_PREFIXES
            .iter()
            .any(|prefix| current.starts_with(prefix) && current.len() > prefix.len() + 8);
        if looks_like_credential {
            out.push_str(REDACTED);
        } else {
            out.push_str(current);
        }
        current.clear();
    };

    for c in text.chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
            current.push(c);
        } else {
            flush(&mut current, &mut out);
            out.push(c);
        }
    }
    flush(&mut current, &mut out);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registered_secret_never_survives() {
        let mut redactor = Redactor::new();
        redactor.register_secret("hunter2-correct-horse");

        let redacted = redactor.redact("connecting with hunter2-correct-horse to the api");
        assert!(!redacted.contains("hunter2-correct-horse"));
        assert!(redacted.contains(REDACTED));
        assert!(redacted.contains("connecting with"));
    }

    #[test]
    fn a_registered_secret_is_removed_everywhere_it_appears() {
        let mut redactor = Redactor::new();
        redactor.register_secret("s3cret-value-here");

        let redacted = redactor.redact("s3cret-value-here and again s3cret-value-here");
        assert!(!redacted.contains("s3cret-value-here"));
        assert_eq!(redacted.matches(REDACTED).count(), 2);
    }

    /// A longer secret that contains a shorter one must be redacted whole,
    /// rather than leaving the surrounding characters exposed.
    #[test]
    fn overlapping_secrets_redact_longest_first() {
        let mut redactor = Redactor::new();
        redactor.register_secret("abcdefgh");
        redactor.register_secret("abcdefghijklmnop");

        let redacted = redactor.redact("token abcdefghijklmnop end");
        assert!(!redacted.contains("ijklmnop"));
        assert_eq!(redacted, "token [redacted] end");
    }

    /// Registering a two-character "secret" would turn the log into noise
    /// without protecting anything.
    #[test]
    fn very_short_values_are_not_registered() {
        let mut redactor = Redactor::new();

        assert!(!redactor.register_secret("abc"));
        assert!(!redactor.register_secret(""));
        assert_eq!(redactor.secret_count(), 0);

        assert!(redactor.register_secret("longenoughsecret"));
        assert_eq!(redactor.secret_count(), 1);
    }

    #[test]
    fn vendor_credential_prefixes_are_recognised_without_registration() {
        let redactor = Redactor::new();

        for credential in [
            "sk-ant-api03-abcdefghijklmnop",
            "ghp_abcdefghijklmnopqrstuvwxyz",
            "glpat-abcdefghijklmnop",
            "xoxb-1234567890-abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "AIzaSyA-abcdefghijklmnop",
        ] {
            let text = format!("the agent ran: curl -H 'Bearer {credential}' https://api");
            let redacted = redactor.redact(&text);
            assert!(
                !redacted.contains(credential),
                "{credential} survived redaction: {redacted}"
            );
        }
    }

    #[test]
    fn ordinary_text_that_merely_looks_technical_survives() {
        let redactor = Redactor::new();

        for harmless in [
            "sk-",
            "the build succeeded in 3.2s",
            "reading ~/Downloads/apartments/2026-01.csv",
            "container ephemeral-csv-comparator started on port 8080",
            "AKIA",
        ] {
            assert_eq!(
                redactor.redact(harmless),
                harmless,
                "{harmless:?} should not have been redacted"
            );
        }
    }

    #[test]
    fn values_of_sensitive_keys_are_redacted_whatever_they_look_like() {
        let redactor = Redactor::new();

        for (input, leaked) in [
            ("DB_PASSWORD=tr0ub4dor", "tr0ub4dor"),
            ("api_key=plainlooking123", "plainlooking123"),
            ("Authorization: abcdefghijk", "abcdefghijk"),
            ("{\"token\": \"opaque-value\"}", "opaque-value"),
            ("SESSION=abc123def456", "abc123def456"),
        ] {
            let redacted = redactor.redact(input);
            assert!(
                !redacted.contains(leaked),
                "{input:?} leaked {leaked:?} as {redacted:?}"
            );
        }
    }

    /// Redaction must not destroy the surrounding text, or nobody will be able
    /// to use the log for the thing it exists for.
    #[test]
    fn redaction_preserves_the_readable_parts_of_a_line() {
        let redactor = Redactor::new();
        let redacted = redactor.redact("starting container with DB_PASSWORD=hunter2 and port 8080");

        assert!(redacted.contains("starting container with"));
        assert!(redacted.contains("port 8080"));
        assert!(!redacted.contains("hunter2"));
    }

    #[test]
    fn multi_line_text_is_redacted_line_by_line() {
        let mut redactor = Redactor::new();
        redactor.register_secret("registered-secret-value");

        let redacted = redactor.redact(
            "line one is fine\nDB_PASSWORD=leaked\nregistered-secret-value\nlast line is fine\n",
        );

        assert!(redacted.contains("line one is fine"));
        assert!(redacted.contains("last line is fine"));
        assert!(!redacted.contains("leaked"));
        assert!(!redacted.contains("registered-secret-value"));
    }

    #[test]
    fn is_clean_reports_whether_anything_would_be_redacted() {
        let mut redactor = Redactor::new();
        redactor.register_secret("a-registered-secret");

        assert!(redactor.is_clean("nothing to see here"));
        assert!(!redactor.is_clean("here is a-registered-secret"));
        assert!(!redactor.is_clean("password=anything"));
    }

    #[test]
    fn redact_in_place_matches_redact() {
        let mut redactor = Redactor::new();
        redactor.register_secret("a-registered-secret");

        let mut text = "leaking a-registered-secret here".to_owned();
        redactor.redact_in_place(&mut text);
        assert_eq!(text, "leaking [redacted] here");
    }

    /// The redactor holds secrets, so it must not print them — a Debug output
    /// in a panic message or a stray `dbg!` would defeat the whole exercise.
    #[test]
    fn the_redactor_reports_a_count_rather_than_its_contents() {
        let mut redactor = Redactor::new();
        redactor.register_secret("a-registered-secret");

        assert_eq!(redactor.secret_count(), 1);

        let debugged = format!("{redactor:?}");
        assert!(
            !debugged.contains("a-registered-secret"),
            "Debug leaked a secret: {debugged}"
        );
        assert!(debugged.contains("secret_count: 1"), "{debugged}");
    }
}
