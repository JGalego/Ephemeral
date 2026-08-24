//! # Ephemeral engine
//!
//! The operations that need *this machine*: a container runtime to build and
//! run an application, and a model provider to write one.
//!
//! ## Why this is not in `ephemeral-api`
//!
//! [`ephemeral-api`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-api)
//! is the service layer every client shares, and it holds no I/O of its own on
//! purpose: it compiles for a phone, where there is no daemon and no
//! subprocess. Generating talks to a model and running needs a container, so
//! neither belongs there.
//!
//! For as long as there was one client with a daemon, that left them in the
//! terminal — which was fine until a second client wanted to run an application
//! too. A window with its own copy of "plan, generate, build, repair, record"
//! would be the second, subtly different Ephemeral that the service layer
//! exists to prevent, and the difference would show up as two applications with
//! the same name behaving differently depending on which client started them.
//!
//! So: **`ephemeral-api` is what every client can do; this is what a client
//! with a machine underneath it can do.** The CLI and the desktop window both
//! call this, and neither sequences these steps itself.
//!
//! ## What comes out of it
//!
//! Data, and sentences already phrased for a person — the same rule the views
//! follow. Nothing here prints, formats for a terminal, or knows what a button
//! is; a client decides how to draw what it is told. Where an operation has
//! something a person needs to know — that a capability they granted is doing
//! nothing, that the paths to pass are not the paths on this machine — the
//! sentence comes from here so that both clients say it the same way.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod container;
pub mod generation;
pub mod reach;
pub mod sandbox;

pub use container::{
    Orphan, Ran, Reconciled, Started, Sweep, orphans, output, pause, reconcile, remove_orphans,
    resume, run_once, start, stop, sweep,
};
pub use generation::{Generated, PROVIDERS, Requested, generate, models, provider_authority};
pub use sandbox::{Confinement, specification};

/// One thing checked about this machine, and what to do if it is wrong.
///
/// A diagnostic that only reports symptoms wastes somebody's time, so every
/// check carries its remedy. Shared by the terminal's `doctor` and the window,
/// because a machine that is fine in one and broken in the other would be worse
/// than either answer alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Check {
    /// What was checked, phrased as the finding.
    pub what: String,

    /// `Some(true)` fine, `Some(false)` broken, `None` worth knowing about.
    ///
    /// Three states rather than two, because an absent container runtime is
    /// neither: everything except running an application works without one, and
    /// reporting that as a failure teaches people to ignore the output.
    pub ok: Option<bool>,

    /// What would fix it, when something needs fixing.
    pub advice: Option<String>,
}

/// What this machine can and cannot do right now.
///
/// Deliberately not a pass/fail: it answers "why did nothing happen", which on
/// a new installation is usually a permission Ephemeral has not been given
/// rather than anything broken.
#[must_use]
pub fn diagnostics(workspace: &ephemeral_core::storage::Workspace) -> Vec<Check> {
    use ephemeral_runtime::Runtime as _;

    let mut checks = Vec::new();

    let availability = ephemeral_runtime::docker::DockerRuntime::new().availability();
    checks.push(Check {
        what: availability.explanation.clone(),
        ok: availability.usable.then_some(true),
        advice: (!availability.usable).then(|| {
            "Everything except building and running an application works without one.".to_owned()
        }),
    });

    for (permission, what_for) in [
        (
            ephemeral_api::authority::RUNTIME,
            "build and run applications in containers",
        ),
        (
            ephemeral_api::authority::HOSTED_PROVIDER,
            "generate with a hosted model",
        ),
        (
            ephemeral_api::authority::CREDENTIAL,
            "use a model provider's credential",
        ),
    ] {
        let granted = ephemeral_api::authority::require(workspace.ledger(), &permission).is_ok();
        checks.push(Check {
            what: format!(
                "Ephemeral {} {what_for}",
                if granted { "may" } else { "may not" }
            ),
            ok: granted.then_some(true),
            advice: (!granted).then(|| {
                ephemeral_api::authority::grant_argument(&permission).map_or_else(
                    || "You can allow it in Ephemeral's own permissions.".to_owned(),
                    |written| format!("`ephemeral grant ephemeral {written}` allows it."),
                )
            }),
        });
    }

    match workspace.audit().verify() {
        Ok(()) => checks.push(Check {
            what: format!(
                "the security record is intact ({} entries)",
                workspace.audit().len()
            ),
            ok: Some(true),
            advice: None,
        }),
        Err(error) => checks.push(Check {
            what: format!("the security record has been altered: {error}"),
            ok: Some(false),
            advice: Some("Treat this as a security event rather than a bug.".to_owned()),
        }),
    }

    checks
}
