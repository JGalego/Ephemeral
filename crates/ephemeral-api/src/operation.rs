//! Things a client asks Ephemeral to *do*, rather than to show.
//!
//! An operation here is the whole of what happens, not a helper a client calls
//! partway through. Creating an application means validating the intent,
//! deriving a name, writing the manifest, preparing its storage and recording
//! the act in the audit log — and a client that performed four of those five
//! would have produced an application that exists with no record of anyone
//! having asked for it.
//!
//! That is not hypothetical. Creation was written three times — once in the
//! CLI, once in the C ABI, and nearly once more in the desktop window — and the
//! C ABI's copy silently omitted the audit entry. Three implementations of one
//! operation is exactly the second, subtly different Ephemeral this crate
//! exists to prevent, so there is one.
//!
//! Nothing here decides anything. Permissions, lifecycle transitions and paths
//! are all `ephemeral-core`'s, unchanged.

use ephemeral_core::{
    Actor, AppId,
    audit::AuditEvent,
    manifest::{AppManifest, Metadata},
    retention::RetentionPolicy,
    storage::{AppStore as _, Workspace},
};

/// Why an operation could not be carried out, phrased for a person.
///
/// A string because every client shows it to somebody, and the core's own
/// messages are already written to be read. A second vocabulary on this side is
/// how a window ends up explaining things differently from a terminal.
pub type Failure = String;

/// Records a new application from what somebody typed.
///
/// Nothing is generated, built or run: this is the act of asking. The
/// application lands in [`LifecycleState::Requested`](ephemeral_core::lifecycle::LifecycleState::Requested),
/// which is a state the lifecycle already models and every client already
/// renders.
///
/// `name` overrides the name derived from the intent. Most callers pass `None`
/// — a name somebody has to invent before they can describe what they want is a
/// question asked too early.
///
/// # Errors
///
/// If the intent is empty, if the manifest cannot be written, if its storage
/// cannot be prepared, or if the workspace cannot be saved.
pub fn create(
    workspace: &mut Workspace,
    intent: &str,
    name: Option<&str>,
    retention: RetentionPolicy,
) -> Result<AppManifest, Failure> {
    let intent = intent.trim();
    if intent.is_empty() {
        return Err("tell me what you want the application to do".to_owned());
    }

    let name = name.map_or_else(|| derive_name(intent), str::to_owned);
    let id = AppId::generate(&name);

    let mut manifest = AppManifest::requested(id.clone(), &name);
    intent.clone_into(&mut manifest.description);
    manifest.metadata = Metadata::for_intent(intent, retention);

    workspace
        .apps_mut()
        .create(&manifest)
        .map_err(|error| format!("could not save {id}: {error}"))?;
    workspace
        .apps()
        .prepare(&id)
        .map_err(|error| format!("could not create the storage for {id}: {error}"))?;

    // Before the save, so that a workspace which fails to save has not recorded
    // a creation that did not happen.
    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppCreated {
            app: id,
            purpose: intent.to_owned(),
        },
    );

    workspace
        .save()
        .map_err(|error| format!("could not save: {error}"))?;

    Ok(manifest)
}

/// A readable name from what somebody typed.
///
/// Four words, because a name is a label in a list rather than a summary, and
/// anything that does not read as a name at a glance has failed at being one.
#[must_use]
pub fn derive_name(intent: &str) -> String {
    let words: Vec<&str> = intent
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .take(4)
        .collect();

    if words.is_empty() {
        return "App".to_owned();
    }

    words
        .iter()
        .map(|word| {
            let cleaned: String = word
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect();
            let mut characters = cleaned.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::lifecycle::LifecycleState;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let home = tempfile::tempdir().expect("a temporary home");
        let workspace = Workspace::open(home.path()).expect("a workspace");
        (home, workspace)
    }

    #[test]
    fn a_name_reads_like_a_name() {
        assert_eq!(
            derive_name("compare these two CSV files and show me the differences"),
            "Compare These Two CSV"
        );
        assert_eq!(derive_name("build a todo list"), "Build A Todo List");
        assert_eq!(derive_name("!!! ???"), "App");
        assert_eq!(derive_name(""), "App");
    }

    #[test]
    fn creating_records_the_intent_verbatim() {
        let (_home, mut space) = workspace();

        let manifest = create(
            &mut space,
            "  count the words in a file  ",
            None,
            RetentionPolicy::default(),
        )
        .expect("an application");

        // Trimmed, but otherwise exactly what was typed: the description is
        // what a person will be shown back, and paraphrasing it here is how a
        // window ends up disagreeing with what somebody asked for.
        assert_eq!(manifest.description, "count the words in a file");
        assert_eq!(manifest.lifecycle.state(), LifecycleState::Requested);
    }

    /// An application that exists with no record of anyone having asked for it
    /// is precisely the hole the audit log is there to close — and the C ABI's
    /// own copy of this operation had it.
    #[test]
    fn creating_is_recorded_in_the_audit_log() {
        let (_home, mut space) = workspace();

        let manifest = create(
            &mut space,
            "count the words in a file",
            None,
            RetentionPolicy::default(),
        )
        .expect("an application");

        let recorded = crate::recent_activity(space.audit(), Some(&manifest.id), 10);
        assert!(
            !recorded.is_empty(),
            "creating an application left no trace in the audit log"
        );
    }

    #[test]
    fn an_empty_intent_is_refused() {
        let (_home, mut space) = workspace();

        let refused = create(&mut space, "   ", None, RetentionPolicy::default());

        assert!(refused.is_err(), "an application was created from nothing");
    }

    #[test]
    fn a_given_name_wins_over_a_derived_one() {
        let (_home, mut space) = workspace();

        let manifest = create(
            &mut space,
            "count the words in a file",
            Some("Word Counter"),
            RetentionPolicy::default(),
        )
        .expect("an application");

        assert_eq!(manifest.name, "Word Counter");
    }
}
