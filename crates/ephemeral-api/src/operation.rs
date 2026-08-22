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
//! Rolling back is the same story told twice. It moves source on disk, appends
//! to a manifest's history, withdraws grants the older version must not
//! inherit, and records all of it — four steps that are one act, and a client
//! that performed three of them would leave an application running code nobody
//! approved for it.
//!
//! Nothing here decides anything. Permissions, lifecycle transitions and paths
//! are all `ephemeral-core`'s, unchanged.

use serde::{Deserialize, Serialize};

use ephemeral_core::{
    Actor, AppId, PermissionDelta, Principal,
    audit::AuditEvent,
    manifest::{AppManifest, Metadata},
    permission::Permission,
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

/// What a rollback did, and what a person needs told about it.
///
/// The sentences are here rather than in each client for the reason every other
/// phrase in this crate is: a rollback that withdrew three grants must say so
/// the same way in a window as in a terminal, and two clients composing that
/// sentence from parts is how the two start disagreeing about what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rollback {
    /// Which application.
    pub app: String,

    /// The human-facing number of the version returned to.
    ///
    /// The version *rolled to*, not the entry this rollback appended: the
    /// history keeps both, and "back to version 2" is the sentence a person can
    /// check against what they were shown.
    pub sequence: u32,

    /// The abbreviated digest of the version returned to.
    pub digest: String,

    /// How many grants were withdrawn because this version asks for more than
    /// the one it replaced.
    pub grants_withdrawn: usize,

    /// How many capabilities this version asks for that the newer one did not.
    pub newly_requested: usize,

    /// What happened, in a sentence.
    pub headline: String,

    /// What the person now has to decide, if a rollback widened what is asked
    /// for. `None` when nothing was withdrawn, because a caution nobody needs
    /// is a caution nobody reads.
    pub caution: Option<String>,

    /// Why the built image is gone.
    pub note: String,
}

/// Returns an application to a version it used to be.
///
/// `version` is matched by prefix against the digests this application actually
/// recorded, never built from the string: a digest that is not in its history
/// is not a version of it, whatever else it might be a digest of.
///
/// Four things happen together or not at all — the source on disk goes back,
/// the manifest records the change, the built image is cleared, and any
/// capability the older version asks for that the newer one had stopped needing
/// has its grant withdrawn. The last of those is the one that matters most:
/// doing the first three without it would hand an application a permission on
/// the strength of an approval given for different code
/// ([ADR-0011](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md)).
///
/// # Errors
///
/// If there is no such application, if `version` matches no recorded version or
/// more than one, if that version's source is not on this machine, if the
/// application is running, or if anything cannot be saved.
pub fn rollback(
    workspace: &mut Workspace,
    app: &AppId,
    version: &str,
) -> Result<Rollback, Failure> {
    let mut manifest = workspace
        .apps()
        .load(app)
        .map_err(|_| format!("there is no application called {app}."))?;

    let (digest, target_sequence) = resolve(&manifest, version)?;

    // Asked before anything moves. A version whose source was never kept — one
    // recorded before snapshots existed, or swept away by retention — can be
    // described and not restored, and saying so beats a half-done rollback.
    if !workspace.apps().has_version(app, &digest) {
        return Err(format!(
            "version {} of {app} is recorded but its source is not on this machine, \
             so there is nothing to go back to.",
            digest.short()
        ));
    }

    if manifest.lifecycle.state().is_runnable() {
        return Err(format!(
            "{app} is {}. Stop it first — rolling back changes the code it would run.",
            manifest.lifecycle.state().headline().to_lowercase()
        ));
    }

    // The manifest first: it refuses a version that is already the current one,
    // and there is no point moving files for a rollback that will be rejected.
    let delta = manifest
        .revert_to(&digest)
        .map_err(|error| format!("cannot roll {app} back: {error}"))?;

    workspace
        .apps()
        .restore_version(app, &digest)
        .map_err(|error| format!("could not restore the source of {app}: {error}"))?;

    // The question ADR-0011 exists to answer, reached from the other direction:
    // rolling *back* widens when the version being left behind had dropped a
    // capability, and the older version must not inherit an approval given
    // while it was not being asked for.
    let grants_withdrawn = if delta.widens() {
        withdraw_widened(workspace, app, &delta)
    } else {
        0
    };

    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppRolledBack {
            app: app.clone(),
            to: digest.as_str().to_owned(),
            grants_revoked: grants_withdrawn,
        },
    );

    workspace
        .apps_mut()
        .save(&manifest)
        .map_err(|error| format!("could not save {app}: {error}"))?;
    workspace
        .save()
        .map_err(|error| format!("could not save: {error}"))?;

    Ok(Rollback {
        app: app.to_string(),
        sequence: target_sequence,
        digest: digest.short().to_owned(),
        grants_withdrawn,
        newly_requested: delta.added.len(),
        headline: format!(
            "Rolled {app} back to version {target_sequence} ({}).",
            digest.short()
        ),
        caution: (grants_withdrawn > 0).then(|| {
            format!(
                "This version asks for {} thing(s) the one it replaced had stopped needing, \
                 so {grants_withdrawn} grant(s) were withdrawn. Look at what it asks for now \
                 and allow again only what you still want.",
                delta.added.len()
            )
        }),
        note: "The built image was cleared: a version is its source, and running the newer \
               build under this version's name would report one thing and run another. \
               Generate again to rebuild."
            .to_owned(),
    })
}

/// The one version `version` names, or why it names none.
fn resolve(
    manifest: &AppManifest,
    version: &str,
) -> Result<(ephemeral_core::VersionDigest, u32), Failure> {
    let matches: Vec<_> = manifest
        .versions
        .iter()
        .filter(|recorded| recorded.digest.matches(version))
        .map(|recorded| (recorded.digest.clone(), recorded.sequence))
        .collect();

    match matches.as_slice() {
        [one] => Ok(one.clone()),
        // Naming what it does have, rather than a command to run: this answer
        // is read in a window as often as in a terminal, and a client's own
        // instructions are not this crate's to give.
        [] => Err(format!(
            "{} has no version matching {version:?}. It has {}.",
            manifest.id,
            recorded_versions(manifest)
        )),
        many => Err(format!(
            "{version:?} matches {} versions of {}. More of the digest picks one.",
            many.len(),
            manifest.id
        )),
    }
}

/// The versions an application actually has, newest first, for an error that
/// would otherwise leave somebody guessing.
fn recorded_versions(manifest: &AppManifest) -> String {
    if manifest.versions.is_empty() {
        return "none: it has never been generated".to_owned();
    }

    let listed: Vec<String> = manifest
        .versions
        .iter()
        .rev()
        .take(LISTED_VERSIONS)
        .map(|version| format!("{} ({})", version.sequence, version.digest.short()))
        .collect();

    let more = manifest.versions.len().saturating_sub(listed.len());
    let tail = if more > 0 {
        format!(", and {more} older")
    } else {
        String::new()
    };

    format!("{}{tail}", listed.join(", "))
}

/// How many versions an error lists before it starts summarising.
///
/// An error is read, not scrolled: an application generated forty times should
/// not answer a mistyped digest with forty digests.
const LISTED_VERSIONS: usize = 5;

/// Withdraws grants that a widening change would otherwise silently inherit.
///
/// Returns how many were withdrawn. Only the ones the *new* request touches: an
/// update that adds network access does not cost the user the file access they
/// already agreed to.
///
/// Shared by generation and rollback because it is the same rule reached from
/// two directions — an approval given for one version's code is not an approval
/// for another's, whichever direction the version moved.
pub fn withdraw_widened(workspace: &mut Workspace, app: &AppId, delta: &PermissionDelta) -> usize {
    let subject = Principal::app(app.clone());
    let mut withdrawn = 0;

    for permission in &delta.added {
        withdrawn += workspace
            .ledger_mut()
            .revoke(&subject, &Permission::App(permission.clone()), Actor::User)
            .unwrap_or(0);
    }

    withdrawn
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
