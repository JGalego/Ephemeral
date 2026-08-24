//! Running an application on the device that holds it.
//!
//! Until this existed, a phone could describe an application, plan it, generate
//! it, record its permissions and show its history — and then had to hand it to
//! somebody else's computer to find out what it did. That was not a limitation
//! of handsets; it was a limitation of Docker, which a handset cannot have
//! ([ADR-0021]).
//!
//! There is almost nothing here, on purpose. The sequence a run goes through
//! lives in [`ephemeral_runtime::wasm::run_application`], where every client
//! calls the same copy of it — a phone and a terminal each composing the same
//! five steps are two subtly different Ephemerals, and the step one of them
//! gets wrong is the one nobody compares. What this module supplies is the two
//! answers only a phone knows: what the ledger says, and what `~` means on a
//! device that has no home directory.
//!
//! [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md

use ephemeral_core::{
    AppId,
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::wasm::{HANDHELD_CEILING, Runnable};
use serde::Serialize;

/// What one run produced, as a host reads it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Ran {
    /// Whether it did what it was asked.
    pub succeeded: bool,

    /// How it exited. 124 is having used its whole processing allowance and
    /// 137 is having asked for too much memory, as they would be from a
    /// container.
    pub exit_code: i32,

    /// Everything it printed, both streams, whole.
    pub output: String,

    /// How the output is meant to be shown: `"page"` or `"text"`.
    pub presentation: &'static str,

    /// Access the person granted that this runtime will not give effect to.
    pub refused: Vec<String>,

    /// What the person allowed that **Ephemeral itself** may not carry out.
    ///
    /// Not the same as `refused`, and on a phone this is the one that matters:
    /// nothing here mirrors the operating system's own permissions into the
    /// ledger yet, so an application allowed to read a folder can still be
    /// holding a grant Ephemeral has no authority to act on. The terminal and
    /// the window have always shown this before a run. This did not, which
    /// left a handset the only client where an application could find nothing
    /// and nothing said why.
    pub inert: Option<String>,
}

/// Runs one application here, under exactly what it was granted.
///
/// `arguments` is what the domain composed from the form — never anything this
/// crate assembled, because a phone, a window and a terminal building argument
/// vectors separately are three subtly different applications.
pub(crate) fn run(
    workspace: &Workspace,
    app: &AppId,
    arguments: Vec<String>,
    reach: std::sync::Arc<dyn ephemeral_runtime::wasm::Reach>,
) -> Result<Ran, String> {
    let manifest = workspace
        .apps()
        .load(app)
        .map_err(|_| format!("there is no application called {app}"))?;

    // From the ledger, never from the manifest. The manifest records what the
    // application wants, and building the sandbox from it would let an
    // application widen its own confinement by asking.
    let held = ephemeral_api::authority::grants(workspace.ledger(), app);
    let inert = held.explain_inert();
    let granted = held.effective();

    let ran = ephemeral_runtime::wasm::run_application(&Runnable {
        manifest: &manifest,
        layout: workspace.layout(),
        granted: &granted,
        // A handset has no user home for `~` to mean, and the honest stand-in
        // is Ephemeral's own root: a scope written against `~` then resolves
        // inside what this application already owns rather than to nothing, or
        // worse, to somewhere it was never meant to reach.
        home: workspace.layout().root().to_path_buf(),
        arguments,
        // Somebody is holding the device and waiting.
        ceiling: HANDHELD_CEILING,
        // The platform's own HTTPS, which is the only one worth having here.
        // Supplying it grants nothing: an application reaches a destination
        // only if a person allowed that destination.
        reach: Some(reach),
    })
    .map_err(|error| error.to_string())?;

    Ok(Ran {
        succeeded: ran.completed.succeeded,
        exit_code: ran.completed.exit_code,
        output: ran.completed.output,
        presentation: ran.shown.as_str(),
        refused: ran.refused,
        inert,
    })
}
