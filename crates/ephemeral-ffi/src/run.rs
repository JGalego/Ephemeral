//! Running an application on the device that holds it.
//!
//! Until this existed, a phone could describe an application, plan it, generate
//! it, record its permissions and show its history — and then had to hand it to
//! somebody else's computer to find out what it did. That was not a limitation
//! of handsets; it was a limitation of Docker, which a handset cannot have
//! ([ADR-0021]).
//!
//! Everything here is ordinary domain work, in the ordinary order:
//!
//! 1. what the manifest says the application is,
//! 2. what the **ledger** says it was granted — never the manifest, which is
//!    what it *asked* for,
//! 3. what that grant becomes as a sandbox,
//! 4. what there is to run,
//! 5. the run itself.
//!
//! Steps two and three are the same functions the desktop calls. A phone that
//! computed its own confinement would be a second Ephemeral with a second set
//! of bugs, and the interesting bugs in a sandbox are the ones only one of two
//! implementations has.
//!
//! [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md

use std::time::Duration;

use ephemeral_core::{
    AppId,
    manifest::RuntimeKind,
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::{
    ContainerSpec, HostPaths, Secrets,
    wasm::{HANDHELD_CEILING, Program, WasmRuntime},
};
use serde::Serialize;

/// What one run produced.
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

    /// Access the person granted that this runtime will not give effect to.
    ///
    /// Said out loud rather than dropped. Somebody who allowed an application
    /// to read a folder and sees it fail to find the folder is owed the reason,
    /// and the reason is us.
    pub refused: Vec<String>,
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
) -> Result<Ran, String> {
    let manifest = workspace
        .apps()
        .load(app)
        .map_err(|_| format!("there is no application called {app}"))?;

    let runtime = manifest.runtime.as_ref().ok_or_else(|| {
        format!(
            "{app} has not been generated yet, so there is nothing to run. \
             Describe it and generate it first."
        )
    })?;

    if runtime.kind != RuntimeKind::Wasm {
        return Err(format!(
            "{app} runs on {}, which this device does not have. \
             WebAssembly applications run here; anything else needs a computer with Docker.",
            runtime.kind
        ));
    }

    // From the ledger, never from the manifest. The manifest records what the
    // application wants, and building the sandbox from it would let an
    // application widen its own confinement by asking.
    let granted = ephemeral_api::authority::grants(workspace.ledger(), app).effective();

    // Its own storage, which every application has and nothing has to grant.
    // Created here rather than assumed: an application generated before this
    // directory was part of the layout, or restored from a recipe, has never
    // had one, and "no such file or directory" is a terrible way to learn that.
    let data = workspace.layout().app(app).data();
    std::fs::create_dir_all(&data)
        .map_err(|error| format!("{app} has nowhere to keep its own files: {error}"))?;

    let paths = HostPaths {
        // A handset has no user home for `~` to mean, and the honest stand-in
        // is Ephemeral's own root: a scope written against `~` then resolves
        // inside what this application already owns rather than to nothing, or
        // worse, to somewhere it was never meant to reach.
        home: workspace.layout().root().to_path_buf(),
        data_dir: data,
    };

    let spec = ContainerSpec::from_grants(
        app.clone(),
        // No image. This runtime does not have any, and the field exists for
        // the one that does.
        String::new(),
        arguments,
        manifest.resources,
        &granted,
        &paths,
    )
    .map_err(|error| error.to_string())?;

    let refused = spec
        .refused
        .iter()
        .map(|refusal| format!("Not granting {} — {}", refusal.granted, refusal.reason))
        .collect();

    let program = Program::locate(
        runtime.program.as_deref(),
        &workspace.layout().app(app).source(),
        &workspace.layout().interpreters_dir(),
    )
    .map_err(|error| error.to_string())?;

    let completed = WasmRuntime::new()
        .run_once(&program, &spec, allowance(&manifest), &Secrets::new())
        .map_err(|error| error.to_string())?;

    Ok(Ran {
        succeeded: completed.succeeded,
        exit_code: completed.exit_code,
        output: completed.output,
        refused,
    })
}

/// How long this application may run here.
///
/// The smaller of what the manifest declares and what a person will wait for.
/// A manifest written for a desktop may say fifteen minutes, which on a phone
/// is not a long job but a frozen application.
fn allowance(manifest: &ephemeral_core::AppManifest) -> Duration {
    let declared = manifest
        .resources
        .max_runtime
        .map_or(HANDHELD_CEILING, |period| {
            // Negative or zero is not a shorter allowance, it is a nonsense one, and
            // the safe reading of nonsense is the ceiling rather than nothing.
            u64::try_from(period.as_seconds()).map_or(HANDHELD_CEILING, Duration::from_secs)
        });

    declared.min(HANDHELD_CEILING)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest written for a desktop does not get a desktop's patience.
    #[test]
    fn a_long_declared_runtime_is_cut_down_to_what_somebody_will_wait_for() {
        let mut manifest = ephemeral_core::AppManifest::requested(
            AppId::parse("tally").expect("a valid id"),
            "Tally",
        );
        manifest.resources.max_runtime =
            Some(ephemeral_core::retention::RetentionPeriod::seconds(900));

        assert_eq!(allowance(&manifest), HANDHELD_CEILING);
    }

    /// And an application that asked for less keeps the smaller number. A
    /// ceiling is not a target.
    #[test]
    fn an_application_that_asked_for_less_is_given_less() {
        let mut manifest = ephemeral_core::AppManifest::requested(
            AppId::parse("tally").expect("a valid id"),
            "Tally",
        );
        manifest.resources.max_runtime =
            Some(ephemeral_core::retention::RetentionPeriod::seconds(5));

        assert_eq!(allowance(&manifest), Duration::from_secs(5));
    }
}
