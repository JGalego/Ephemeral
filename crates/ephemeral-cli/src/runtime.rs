//! `ephemeral run`, `stop`, `pause`, `resume`, `status`, `watch` and `cleanup`
//! — the terminal's half of putting an application into a sandbox.
//!
//! Everything that decides anything is in
//! [`ephemeral_engine`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-engine),
//! which the desktop window calls too. What is left here is resolving what
//! somebody typed into an application, and drawing what the engine says
//! happened. A client that sequenced these steps itself would be a second,
//! subtly different Ephemeral — and the difference would show up as the same
//! application behaving differently depending on which one started it.

use std::path::Path;

use anyhow::{Context as _, Result};
use ephemeral_core::{manifest::RuntimeKind, storage::Workspace};
use ephemeral_engine::container;
use ephemeral_runtime::wasm::Shown;

use crate::output;

/// Starts an application.
///
/// Two shapes, chosen by what the application runs on rather than by a flag. A
/// container is started and left running, and `ephemeral status` says what it
/// is doing. A WebAssembly module runs to completion and prints what it
/// produced — there is nothing left to ask about afterwards, so asking somebody
/// to type a second command to see the answer would be silly.
pub(crate) fn run(home: &Path, reference: &str, arguments: &[String]) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    if manifest
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.kind == RuntimeKind::Wasm)
    {
        return run_to_completion(&mut workspace, &mut manifest, arguments);
    }

    let started = container::start(
        &mut workspace,
        &mut manifest,
        arguments,
        "started from the command line",
    )?;

    // Said before the sandbox is described, because it changes what the
    // description means: an application whose grants are inert is confined to
    // nothing for a reason that is not the sandbox working.
    if let Some(inert) = &started.inert {
        println!("{} {inert}", output::warn("Careful:"));
    }
    for refusal in &started.refused {
        println!("{}", output::warn(refusal));
    }

    println!(
        "{} {} is {}.",
        output::good("Started."),
        manifest.id,
        output::state(manifest.lifecycle.state())
    );

    if let Some(id) = &started.container {
        println!(
            "{}",
            output::dim(&format!("Container {}", &id[..id.len().min(12)]))
        );
    }

    for line in &started.confinement {
        println!("{}", output::dim(line));
    }

    Ok(())
}

/// Runs a WebAssembly application here and prints what it produced.
fn run_to_completion(
    workspace: &mut Workspace,
    manifest: &mut ephemeral_core::AppManifest,
    arguments: &[String],
) -> Result<()> {
    let ran = container::run_once(workspace, manifest, arguments, "run from the command line")?;

    // Before the output, because it changes what the output means: an
    // application that found nothing may have found nothing because it was
    // never given anywhere to look.
    if let Some(inert) = &ran.inert {
        println!("{} {inert}", output::warn("Careful:"));
    }
    for refusal in &ran.refused {
        println!("{}", output::warn(refusal));
    }

    if !ran.completed.output.is_empty() {
        print!("{}", ran.completed.output);
        if !ran.completed.output.ends_with('\n') {
            println!();
        }
    }

    if ran.shown == Shown::Page {
        // Said rather than rendered. A terminal is not a browser, and printing
        // markup as though it were the answer would be worse than saying what
        // it is.
        println!(
            "{}",
            output::dim("That is a page. A window or a phone renders it.")
        );
    }

    // What it could reach, after the answer rather than before it: a run that
    // is already over is read for its result first, where a container that has
    // just started is read for what it is now able to do.
    for line in &ran.confinement {
        println!("{}", output::dim(line));
    }

    if ran.completed.succeeded {
        println!("{} {} finished.", output::good("Ran."), manifest.id);
        return Ok(());
    }

    // A non-zero exit is the application's answer, not Ephemeral's failure, so
    // this reports rather than returns an error — and the output above is the
    // part that matters.
    println!(
        "{} {} exited with {}.",
        output::warn("Failed."),
        manifest.id,
        ran.completed.exit_code
    );
    Ok(())
}

/// Stops a running application.
pub(crate) fn stop(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    container::stop(
        &mut workspace,
        &mut manifest,
        "stopped from the command line",
    )?;

    println!(
        "{} {} is no longer running.",
        output::good("Stopped."),
        manifest.id
    );
    Ok(())
}

/// Suspends a running application without losing its state.
pub(crate) fn pause(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    container::pause(
        &mut workspace,
        &mut manifest,
        "paused from the command line",
    )?;

    println!("{} {} is suspended.", output::good("Paused."), manifest.id);
    println!(
        "{}",
        output::dim(&format!(
            "It is still holding memory and its resource limits still apply. \
             `ephemeral resume {}` picks it back up.",
            manifest.id
        ))
    );
    Ok(())
}

/// Picks a suspended application back up.
pub(crate) fn resume(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    container::resume(
        &mut workspace,
        &mut manifest,
        "resumed from the command line",
    )?;

    println!(
        "{} {} is running again.",
        output::good("Resumed."),
        manifest.id
    );
    Ok(())
}

/// Shows what the application itself has printed, when there is one to ask.
///
/// Best-effort by design. This is extra context on the end of a history that is
/// already useful, so a missing runtime or a container that has gone is a quiet
/// omission rather than a failure of `ephemeral logs`.
pub(crate) fn print_output(manifest: &ephemeral_core::AppManifest, lines: u32) {
    let Some(output) = container::output(manifest, lines) else {
        return;
    };

    println!();
    println!(
        "{}",
        output::heading(&format!("{} — output", manifest.name))
    );
    println!();

    if output.trim().is_empty() {
        println!("{}", output::dim("It has not printed anything."));
        return;
    }

    print!("{output}");
}

/// Brings an application's recorded state back in line with its container.
pub(crate) fn status(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    let reconciled = container::reconcile(&mut workspace, &mut manifest)?;

    if !reconciled.holds_a_container {
        println!("{} is {}.", manifest.id, output::state(reconciled.was));
        println!("{}", output::dim("It is not holding a container."));
        return Ok(());
    }

    if !reconciled.changed() {
        println!(
            "{} is {}, and its container agrees.",
            manifest.id,
            output::state(reconciled.now)
        );
        return Ok(());
    }

    println!(
        "{} {} was {}, and is now {}.",
        output::warn("Updated."),
        manifest.id,
        output::state(reconciled.was),
        output::state(reconciled.now)
    );
    if let Some(because) = &reconciled.because {
        println!("{}", output::dim(because));
    }
    Ok(())
}

/// Stops whatever is running on a permission that was just taken back.
pub(crate) fn stop_what_lost_a_permission(
    workspace: &mut Workspace,
    subject: &ephemeral_core::Principal,
) -> Result<Vec<ephemeral_core::AppId>> {
    container::stop_what_lost_a_permission(workspace, subject)
}

/// Containers Ephemeral is holding that no application accounts for.
pub(crate) fn orphans(
    workspace: &Workspace,
    runtime: &ephemeral_runtime::docker::DockerRuntime,
) -> Result<Vec<container::Orphan>> {
    container::orphans(workspace, runtime)
}

/// Watches running applications and acts on what it sees.
///
/// A one-shot command notices a crash the next time somebody asks. This notices
/// it while it is happening, which is also the only way a wall-clock limit can
/// be more than a number in a manifest.
///
/// It runs in the foreground and is stopped with Ctrl-C. That is deliberately
/// the least commitment available: nothing here decides whether Ephemeral
/// eventually has a background service, and a desktop shell can host the same
/// sweep without any of it changing.
pub(crate) fn watch(home: &Path, interval_seconds: u64, once: bool) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let runtime = ephemeral_engine::sandbox::usable_runtime(&workspace)?;

    if !once {
        println!(
            "{}",
            output::dim(&format!(
                "Watching every {interval_seconds}s. Ctrl-C to stop."
            ))
        );
    }

    loop {
        let acted = container::sweep(&mut workspace, &runtime)?;

        for action in &acted {
            println!(
                "{} {} — {}",
                output::warn("Acted."),
                action.app,
                action.what
            );
        }

        if once {
            if acted.is_empty() {
                println!("{}", output::good("Everything is as recorded."));
            }
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_secs(interval_seconds));
    }
}

/// Removes every container no application accounts for.
pub(crate) fn cleanup(home: &Path, confirmed: bool) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let runtime = ephemeral_engine::sandbox::usable_runtime(&workspace)?;

    let found = container::orphans(&workspace, &runtime)?;

    if found.is_empty() {
        println!("{}", output::good("Nothing to clean up."));
        println!(
            "{}",
            output::dim(
                "Every container Ephemeral is holding belongs to an application that \
                 should have one."
            )
        );
        return Ok(());
    }

    for orphan in &found {
        println!(
            "  {} {}",
            output::bold(&orphan.container),
            output::dim(&format!("— {}", orphan.reason))
        );
    }
    println!();

    if !confirmed {
        println!(
            "{}",
            output::dim(&format!(
                "{} container(s) would be removed. Run it again with --yes if you mean it.",
                found.len()
            ))
        );
        return Ok(());
    }

    let removed = container::remove_orphans(&mut workspace, &runtime, &found)
        .context("could not remove every leftover container")?;

    println!("{} {removed} container(s) removed.", output::good("Done."));
    Ok(())
}
