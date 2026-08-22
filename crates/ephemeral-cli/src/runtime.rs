//! `ephemeral run`, `stop`, `pause` and `resume` — the commands that put an
//! application into a sandbox and take it out again.
//!
//! Everything here is bookkeeping around two things it does not decide itself:
//! the lifecycle state machine in `ephemeral-core`, which says whether the
//! transition is legal, and `ephemeral-runtime`, which says what confinement the
//! application gets. The CLI's job is to keep the record honest — to make sure
//! that what the manifest says about an application is what is actually true of
//! the container.
//!
//! The order matters and is deliberate. The user's *intent* is recorded first
//! (`Start`), then the runtime is asked, then the *fact* is recorded by the
//! runtime as actor (`Started` or `StartFailed`). A crash between the two leaves
//! an application in `Starting`, which is a state the machine has, rather than a
//! manifest that claims something untrue.

use std::path::Path;

use anyhow::{Context as _, Error, Result, bail};
use ephemeral_core::{
    Actor, AppId, AppManifest,
    audit::AuditEvent,
    lifecycle::{LifecycleEvent, LifecycleState, TransitionRequest},
    manifest::RuntimeKind,
    permission::AppPermission,
    retention::RetentionPeriod,
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::{
    ContainerSpec, ContainerState, ContainerStatus, HostPaths, ManagedContainer, Runtime as _,
    Secrets, docker::DockerRuntime,
};

use crate::output;

/// Starts an application.
pub(crate) fn run(home: &Path, reference: &str, arguments: &[String]) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    ensure_allowed(&manifest, LifecycleEvent::Start)?;
    let mut spec = specification(&workspace, &manifest)?;

    // Said before it starts, not discovered afterwards. An application whose
    // grants Ephemeral may not carry out runs with less than the person
    // allowed it, and the only other clue is that it can see nothing of theirs
    // — which reads as a sandbox working rather than a permission missing.
    if let Some(explanation) =
        ephemeral_api::authority::grants(workspace.ledger(), &manifest.id).explain_inert()
    {
        println!("{} {}", output::warn("Careful:"), explanation);
    }

    // Appended to the entrypoint rather than replacing it: an application's
    // entry point is part of what it *is*, recorded in its version, and letting
    // a command line replace it would let somebody run something other than the
    // application they are looking at.
    spec.entrypoint.extend(arguments.iter().cloned());
    let runtime = usable_runtime(&workspace)?;

    // Intent first, and only if the state machine allows it. Asking Docker to
    // start something the lifecycle forbids would be an action with no record.
    apply(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Start,
        Actor::User,
        "started from the command line",
    )?;
    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    // Nothing is stored for a generated application's settings yet, so an app
    // granted access to one cannot start. That is a refusal with a reason
    // rather than a silent start with a missing value.
    let started = runtime.start(&spec, &Secrets::new());

    match started {
        Ok(status) => {
            apply(
                &mut workspace,
                &mut manifest,
                LifecycleEvent::Started,
                Actor::Runtime,
                &format!("running in {} as {}", runtime.name(), spec.container_name()),
            )?;

            // What was exposed to the sandbox, recorded at the moment it was
            // exposed. This is the first question an incident review asks, and
            // reconstructing it later from the ledger would answer a different
            // one: what was granted, rather than what was actually mounted.
            workspace
                .audit_mut()
                .append(Actor::Runtime, sandbox_created(runtime.name(), &spec));

            workspace.apps_mut().save(&manifest)?;
            workspace.save()?;

            report_started(&manifest, &spec, status.container_id.as_deref());
            Ok(())
        }
        Err(error) => {
            apply(
                &mut workspace,
                &mut manifest,
                LifecycleEvent::StartFailed,
                Actor::Runtime,
                &error.to_string(),
            )?;
            workspace.apps_mut().save(&manifest)?;
            workspace.save()?;

            Err(anyhow::Error::new(error).context(format!("could not start {}", manifest.id)))
        }
    }
}

/// Stops a running application.
pub(crate) fn stop(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    // What the application is doing is a better answer than what Docker is
    // doing: "it is not running" beats "Docker is not responding" when both
    // are true.
    ensure_allowed(&manifest, LifecycleEvent::Stop)?;
    let runtime = usable_runtime(&workspace)?;

    apply(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Stop,
        Actor::User,
        "stopped from the command line",
    )?;
    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    runtime
        .stop(&manifest.id)
        .with_context(|| format!("could not stop {}", manifest.id))?;

    // The container is gone once it has stopped: everything the application
    // keeps lives in its data directory, which outlives it. Leaving a stopped
    // container behind would make `ephemeral list` and Docker disagree about
    // what exists.
    runtime.remove(&manifest.id).ok();

    apply(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Stopped,
        Actor::Runtime,
        "the container stopped",
    )?;
    workspace.audit_mut().append(
        Actor::Runtime,
        AuditEvent::SandboxDestroyed {
            app: manifest.id.clone(),
            reason: "stopped from the command line".to_owned(),
        },
    );
    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    println!(
        "{} {} is no longer running.",
        output::good("Stopped."),
        manifest.id
    );
    Ok(())
}

/// Whether revoking something from `subject` reaches this application's
/// container.
///
/// Only what is holding one: a revocation changes what the *next* sandbox gets
/// for everything else, and stopping an application that is not running would
/// be an action with no cause.
///
/// Revoking from Ephemeral reaches every one of them, which is the two-tier
/// model's whole point — its authority is what carries every application's
/// capabilities out, so losing it takes the floor out from under all of them at
/// once (ADR-0003).
fn reaches(subject: &ephemeral_core::Principal, manifest: &AppManifest) -> bool {
    if !ephemeral_api::authority::is_running(manifest) {
        return false;
    }

    match subject {
        ephemeral_core::Principal::Ephemeral => true,
        other => other.as_app() == Some(&manifest.id),
    }
}

/// Stops every running application that a revocation just took something from.
///
/// Returns which ones were stopped, so the caller can say so.
///
/// A sandbox is built once, at start. Revoking a grant therefore changes what
/// the *next* container gets and nothing at all about the one already running
/// with what was taken away — the mount stays mounted until the application
/// exits on its own. "Revoked" would be a statement about the future while the
/// present carried on, which is exactly the kind of claim
/// [`SECURITY.md`](https://github.com/JGalego/Ephemeral/blob/main/SECURITY.md)
/// must not make.
///
/// Stopping is the blunt answer and the only honest one available: a container
/// cannot have a mount taken away while it runs, so either it keeps what the
/// person just refused it, or it stops. Revoking Ephemeral's own authority
/// stops everything holding a container, because that authority is what carries
/// every application's capabilities out.
///
/// Deliberately quiet about applications it cannot stop: if the runtime is
/// unreachable the revocation still stands in the ledger, and refusing to
/// revoke because Docker is down would leave the permission in place, which is
/// the worse of the two failures.
pub(crate) fn stop_what_lost_a_permission(
    workspace: &mut Workspace,
    subject: &ephemeral_core::Principal,
) -> Result<Vec<AppId>> {
    let loaded = workspace.load_all()?;
    let affected: Vec<AppId> = loaded
        .loaded
        .iter()
        .filter(|manifest| reaches(subject, manifest))
        .map(|manifest| manifest.id.clone())
        .collect();

    if affected.is_empty() {
        return Ok(Vec::new());
    }

    let runtime = DockerRuntime::new();
    if !runtime.availability().usable {
        // The revocation stands either way. Refusing it because Docker is not
        // answering would leave the permission in place, which is the worse of
        // the two failures.
        return Ok(Vec::new());
    }

    let mut stopped = Vec::new();
    for id in affected {
        let Ok(mut manifest) = workspace.apps().load(&id) else {
            continue;
        };

        if runtime.stop(&id).is_err() {
            continue;
        }
        runtime.remove(&id).ok();

        // Recorded as the runtime's doing and explained as the user's, which is
        // what happened: they revoked, and the container went.
        for (event, actor) in [
            (LifecycleEvent::Stop, Actor::User),
            (LifecycleEvent::Stopped, Actor::Runtime),
        ] {
            if apply(
                workspace,
                &mut manifest,
                event,
                actor,
                "a permission it was running with was taken back",
            )
            .is_err()
            {
                break;
            }
        }

        workspace.audit_mut().append(
            Actor::User,
            AuditEvent::SandboxDestroyed {
                app: id.clone(),
                reason: "a permission it was running with was taken back".to_owned(),
            },
        );
        workspace.apps_mut().save(&manifest)?;
        stopped.push(id);
    }

    workspace.save()?;
    Ok(stopped)
}

/// Suspends a running application without losing its state.
pub(crate) fn pause(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    // What the application is doing is a better answer than what Docker is
    // doing: "it is not running" beats "Docker is not responding" when both
    // are true.
    ensure_allowed(&manifest, LifecycleEvent::Pause)?;
    let runtime = usable_runtime(&workspace)?;

    apply(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Pause,
        Actor::User,
        "paused from the command line",
    )?;

    runtime
        .pause(&manifest.id)
        .with_context(|| format!("could not pause {}", manifest.id))?;

    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

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

/// Resumes a suspended application.
pub(crate) fn resume(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    // What the application is doing is a better answer than what Docker is
    // doing: "it is not running" beats "Docker is not responding" when both
    // are true.
    ensure_allowed(&manifest, LifecycleEvent::Resume)?;
    let runtime = usable_runtime(&workspace)?;

    apply(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Resume,
        Actor::User,
        "resumed from the command line",
    )?;

    runtime
        .resume(&manifest.id)
        .with_context(|| format!("could not resume {}", manifest.id))?;

    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    println!(
        "{} {} is running again.",
        output::good("Resumed."),
        manifest.id
    );
    Ok(())
}

/// Whether there might be a container holding this application's output.
///
/// Not "is it running". A container outlives the run that ended it: a crashed
/// application still has one, and the last thing it printed is the most useful
/// thing on the machine — the traceback that says *why* it crashed. Asking only
/// for states that hold a container withheld the output at exactly the moment
/// somebody needed it, and handed it over once the application had exited
/// cleanly and had nothing to explain.
///
/// An application that was never built has no container and never had one, so
/// there is nothing to ask about. Everything else is worth asking, and Docker
/// answering "no such container" is a fine answer to have asked for.
fn has_output(manifest: &AppManifest) -> bool {
    manifest.runtime.is_some()
}

/// Shows what the application itself has printed, when there is one to ask.
///
/// Best-effort by design. This is extra context on the end of a history that is
/// already useful, so a missing runtime or a container that has gone is a quiet
/// omission rather than a failure of `ephemeral logs`.
pub(crate) fn print_output(manifest: &AppManifest, lines: u32) {
    if !has_output(manifest) {
        return;
    }

    let runtime = DockerRuntime::new();
    if !runtime.availability().usable {
        return;
    }

    let Ok(output) = runtime.logs(&manifest.id, lines) else {
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
///
/// A one-shot command cannot watch anything, so this is where crash detection,
/// health and clean exits are noticed: the next time somebody asks. The
/// alternative — a manifest that says Running because nothing was there to
/// observe the container dying — is a record that lies by omission.
pub(crate) fn status(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    if !manifest.lifecycle.state().requires_runtime() {
        println!(
            "{} is {}.",
            manifest.id,
            output::state(manifest.lifecycle.state())
        );
        println!("{}", output::dim("It is not holding a container."));
        return Ok(());
    }

    let runtime = usable_runtime(&workspace)?;
    let observed = runtime
        .status(&manifest.id)
        .with_context(|| format!("could not ask about {}", manifest.id))?;

    let steps = implied_events(manifest.lifecycle.state(), &observed);
    if steps.is_empty() {
        println!(
            "{} is {}, and its container agrees.",
            manifest.id,
            output::state(manifest.lifecycle.state())
        );
        return Ok(());
    }

    let before = manifest.lifecycle.state();
    let explanation = steps
        .iter()
        .map(|(_, _, reason)| reason.clone())
        .collect::<Vec<_>>()
        .join(", and ");

    for (event, actor, reason) in steps {
        apply(&mut workspace, &mut manifest, event, actor, &reason)?;
    }
    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    println!(
        "{} {} was {}, and is now {}.",
        output::warn("Updated."),
        manifest.id,
        output::state(before),
        output::state(manifest.lifecycle.state())
    );
    println!("{}", output::dim(&explanation));
    Ok(())
}

/// What the state machine should be told, given what the container is doing.
///
/// Returns an empty list when the record and the container already agree.
///
/// The actor on each step is not decoration. Only the runtime may report an
/// execution *fact* — a crash, a failing health check — and only a person or
/// Ephemeral may express an *intention* like stopping. A clean exit is
/// therefore two steps: Ephemeral decides to stop it, and the runtime confirms
/// it has stopped. Getting this wrong would be caught by the machine, which is
/// the point of the machine.
fn implied_events(
    recorded: LifecycleState,
    observed: &ContainerStatus,
) -> Vec<(LifecycleEvent, Actor, String)> {
    use ContainerState::{Absent, Dead, Exited, Paused, Running};
    use LifecycleState as S;

    let crashed = |reason: String| vec![(LifecycleEvent::RuntimeCrashed, Actor::Runtime, reason)];

    match (recorded, observed.state) {
        // Gone without being stopped. Whether it crashed or was killed from
        // outside, the application is not running and the record said it was.
        (S::Running | S::Unhealthy | S::Paused, Absent) => {
            crashed("the container is gone, and Ephemeral did not stop it".to_owned())
        }

        // A clean exit is the application finishing, not failing. Two steps,
        // because that is what the machine models: an intention and a fact.
        (S::Running | S::Unhealthy, Exited) if observed.succeeded() => vec![
            (
                LifecycleEvent::Stop,
                Actor::Ephemeral,
                "the application finished".to_owned(),
            ),
            (
                LifecycleEvent::Stopped,
                Actor::Runtime,
                "it exited cleanly".to_owned(),
            ),
        ],

        (S::Running | S::Unhealthy, Exited | Dead) => crashed(match observed.exit_code {
            Some(code) => format!("the container exited with code {code}"),
            None => "the container died".to_owned(),
        }),

        // Health is only meaningful while it is up, and only when the image
        // defines a check.
        (S::Running, Running) if observed.is_unhealthy() => vec![(
            LifecycleEvent::HealthDegraded,
            Actor::Runtime,
            "its health check is failing".to_owned(),
        )],
        (S::Unhealthy, Running) if !observed.is_unhealthy() => vec![(
            LifecycleEvent::HealthRestored,
            Actor::Runtime,
            "its health check is passing again".to_owned(),
        )],

        // Somebody paused or unpaused it outside Ephemeral. The container is
        // the fact; the record is what needs correcting.
        (S::Running, Paused) => vec![(
            LifecycleEvent::Pause,
            Actor::Ephemeral,
            "the container was paused from outside Ephemeral".to_owned(),
        )],
        (S::Paused, Running) => vec![(
            LifecycleEvent::Resume,
            Actor::Ephemeral,
            "the container was resumed from outside Ephemeral".to_owned(),
        )],

        _ => Vec::new(),
    }
}

/// Watches running applications and acts on what it sees.
///
/// A one-shot command notices a crash the next time somebody asks. This notices
/// it while it is happening, which is also the only way a wall-clock limit can
/// be more than a number in a manifest.
///
/// It runs in the foreground and is stopped with Ctrl-C. That is deliberately
/// the least commitment available: nothing here decides whether Ephemeral
/// eventually has a background service, and a desktop shell can host this same
/// sweep without any of it changing.
pub(crate) fn watch(home: &Path, interval_seconds: u64, once: bool) -> Result<()> {
    let runtime = usable_runtime(&crate::commands::open(home)?)?;

    if !once {
        println!(
            "{}",
            output::dim(&format!(
                "Watching every {interval_seconds}s. Ctrl-C to stop."
            ))
        );
    }

    loop {
        let acted = sweep(home, &runtime)?;

        if once {
            if acted == 0 {
                println!("{}", output::good("Everything is as recorded."));
            }
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_secs(interval_seconds));
    }
}

/// One pass over every application that should be holding a container.
///
/// Returns how many applications it had to do something about.
fn sweep(home: &Path, runtime: &DockerRuntime) -> Result<usize> {
    let mut workspace = crate::commands::open(home)?;

    let watched: Vec<AppId> = workspace
        .load_all()?
        .loaded
        .iter()
        .filter(|manifest| manifest.lifecycle.state().requires_runtime())
        .map(|manifest| manifest.id.clone())
        .collect();

    let mut acted = 0;

    for id in watched {
        // Reloaded each time: a sweep can take a while, and acting on a
        // manifest that was read before the previous application was dealt
        // with would write back a stale lifecycle.
        let Ok(mut manifest) = workspace.apps().load(&id) else {
            continue;
        };

        let mut changed = false;

        // Ceilings first. An application past one should be stopped even if its
        // container looks perfectly healthy — that is what a ceiling is for.
        let data_dir = workspace.layout().app(&id).data();
        let breach = overran(&manifest, ephemeral_core::now())
            .map(|over| {
                format!(
                    "it reached its {} limit {} ago",
                    manifest
                        .resources
                        .max_runtime
                        .map_or_else(|| "time".to_owned(), RetentionPeriod::describe),
                    RetentionPeriod::seconds(over).describe()
                )
            })
            .or_else(|| {
                overfilled(&manifest, &data_dir).map(|over| {
                    format!(
                        "it is using {over} MiB more disk than its {} MiB limit",
                        manifest.resources.storage_mib
                    )
                })
            });

        if let Some(reason) = breach {
            println!("{} {id} — {reason}", output::warn("Stopping"));

            runtime.stop(&id).ok();
            runtime.remove(&id).ok();

            for (event, actor) in [
                (LifecycleEvent::Stop, Actor::Ephemeral),
                (LifecycleEvent::Stopped, Actor::Runtime),
            ] {
                apply(&mut workspace, &mut manifest, event, actor, &reason)?;
            }
            workspace.audit_mut().append(
                Actor::Ephemeral,
                AuditEvent::SandboxDestroyed {
                    app: id.clone(),
                    reason,
                },
            );
            changed = true;
        } else {
            let observed = runtime.status(&id)?;
            for (event, actor, reason) in implied_events(manifest.lifecycle.state(), &observed) {
                println!("{} {id} — {reason}", output::warn("Noticed"));
                apply(&mut workspace, &mut manifest, event, actor, &reason)?;
                changed = true;
            }
        }

        if changed {
            workspace.apps_mut().save(&manifest)?;
            workspace.save()?;
            acted += 1;
        }
    }

    Ok(acted)
}

/// How much disk an application's own storage is using, in mebibytes.
///
/// Measured rather than asked of Docker: the application's data lives on a host
/// bind mount, so Docker's own storage accounting does not cover the thing that
/// actually grows. Returns `None` if the directory cannot be read, which is a
/// reason to say nothing rather than to stop an application.
fn storage_used_mib(directory: &Path) -> Option<u64> {
    fn total(directory: &Path, budget: &mut u32) -> Option<u64> {
        // A bounded walk. A symlink loop or a pathologically deep tree must not
        // turn a routine check into a hang, and stopping early under-reports —
        // which errs towards leaving an application running.
        if *budget == 0 {
            return None;
        }
        *budget -= 1;

        let mut bytes = 0;
        for entry in std::fs::read_dir(directory).ok()? {
            let entry = entry.ok()?;

            // `symlink_metadata`, so a link out of the directory is counted as
            // a link rather than followed to whatever it points at.
            let metadata = entry.metadata().ok()?;
            if metadata.is_dir() {
                bytes += total(&entry.path(), budget)?;
            } else if metadata.is_file() {
                bytes += metadata.len();
            }
        }
        Some(bytes)
    }

    let mut budget = 10_000_u32;
    total(directory, &mut budget).map(|bytes| bytes / (1024 * 1024))
}

/// How far past its disk ceiling an application is, in mebibytes.
///
/// `None` when it is within it, or when the directory could not be measured.
fn overfilled(manifest: &AppManifest, data_dir: &Path) -> Option<u64> {
    let limit = u64::from(manifest.resources.storage_mib);
    let used = storage_used_mib(data_dir)?;

    // `then`, not `then_some`: the latter evaluates its argument whatever the
    // condition, and an application inside its ceiling would underflow here.
    (used > limit).then(|| used - limit)
}

/// When an application's container was started, according to its own history.
///
/// Read from the manifest rather than from Docker: this is when *Ephemeral*
/// started it, which is the thing the limit was agreed against. Containers
/// never restart themselves (`--restart no`), so the two cannot drift.
fn running_since(manifest: &AppManifest) -> Option<ephemeral_core::Timestamp> {
    manifest
        .lifecycle
        .history()
        .iter()
        .rev()
        .find(|transition| transition.event == LifecycleEvent::Started)
        .map(|transition| transition.at)
}

/// How far past its wall-clock ceiling an application is, in seconds.
///
/// `None` when it has no ceiling, has not started, or is still within it. A
/// ceiling of `None` in the manifest means no limit, which is only appropriate
/// for something the user asked to keep running.
fn overran(manifest: &AppManifest, now: ephemeral_core::Timestamp) -> Option<i64> {
    let limit = manifest.resources.max_runtime?.as_seconds();
    let elapsed = (now - running_since(manifest)?).num_seconds();

    // `then`, not `then_some`, for the same reason as `overfilled`: the
    // subtraction must not happen unless the comparison already said it should.
    (elapsed > limit).then(|| elapsed - limit)
}

/// A container Ephemeral is holding that no application accounts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Orphan {
    /// The container's name.
    pub(crate) container: String,

    /// Why nothing accounts for it.
    pub(crate) reason: String,
}

/// Containers Ephemeral created that no application's state accounts for.
///
/// A crash, a kill, or a purge while something was running all leave a
/// container behind. Nothing else cleans them up: they hold a name, disk, and
/// possibly a mount of the user's files, so leaving them is a resource leak with
/// a security edge rather than untidiness.
///
/// Only containers carrying Ephemeral's own label are considered. Reaping by
/// anything looser would be an efficient way to destroy somebody's unrelated
/// work.
pub(crate) fn orphans(workspace: &Workspace, runtime: &DockerRuntime) -> Result<Vec<Orphan>> {
    let held = runtime
        .managed_containers()
        .context("could not ask the container runtime what it is holding")?;

    Ok(classify(workspace, &held))
}

/// Decides which of the containers Ephemeral is holding are unaccounted for.
///
/// Separated from asking Docker so that the decision — the part that could
/// wrongly reap something — is a pure function with tests, rather than
/// something only exercised on a machine with a daemon.
fn classify(workspace: &Workspace, held: &[ManagedContainer]) -> Vec<Orphan> {
    let mut found = Vec::new();

    for container in held {
        let Some(id) = &container.app else {
            found.push(Orphan {
                container: container.name.clone(),
                reason: "it carries Ephemeral's label but names no application".to_owned(),
            });
            continue;
        };

        let reason = match workspace.apps().load(id) {
            // The application is gone but its container is not.
            Err(_) => Some(format!("{id} no longer exists")),

            // The application exists, but its recorded state says it should not
            // be holding a container. That disagreement is the bug: the manifest
            // is what the user is shown, so the container is what is wrong.
            Ok(manifest) if !manifest.lifecycle.state().requires_runtime() => Some(format!(
                "{id} is {}, which does not hold a container",
                manifest.lifecycle.state().headline().to_lowercase()
            )),

            Ok(_) => None,
        };

        if let Some(reason) = reason {
            found.push(Orphan {
                container: container.name.clone(),
                reason,
            });
        }
    }

    found
}

/// Removes every container no application accounts for.
pub(crate) fn cleanup(home: &Path, confirmed: bool) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let runtime = usable_runtime(&workspace)?;

    let found = orphans(&workspace, &runtime)?;

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
        println!("  {} — {}", orphan.container, output::dim(&orphan.reason));
    }
    println!();

    if !confirmed {
        println!(
            "{} container(s) would be removed. Run it again with --yes if you mean it.",
            found.len()
        );
        return Ok(());
    }

    // Removal goes through the same path a stop does, so it cannot reach a
    // container Ephemeral did not name.
    let mut removed = 0;
    for orphan in &found {
        let Some(id) = orphan
            .container
            .strip_prefix(ephemeral_runtime::CONTAINER_PREFIX)
        else {
            continue;
        };
        let Ok(app) = AppId::parse(id) else { continue };

        runtime
            .remove(&app)
            .with_context(|| format!("could not remove {}", orphan.container))?;

        workspace.audit_mut().append(
            Actor::Ephemeral,
            AuditEvent::SandboxDestroyed {
                app,
                reason: format!("cleaned up: {}", orphan.reason),
            },
        );
        removed += 1;
    }

    workspace.save()?;

    println!("{} {removed} container(s) removed.", output::good("Done."));
    Ok(())
}

/// The record of what a sandbox was actually given.
fn sandbox_created(runtime: &str, spec: &ContainerSpec) -> AuditEvent {
    AuditEvent::SandboxCreated {
        app: spec.app.clone(),
        runtime: runtime.to_owned(),
        image: Some(spec.image.clone()),
        mounts: spec
            .mounts
            .iter()
            .map(|mount| {
                format!(
                    "{} ({})",
                    mount.host_path.display(),
                    if mount.writable {
                        "read and write"
                    } else {
                        "read only"
                    }
                )
            })
            .collect(),
        ports: spec.ports.iter().map(|port| port.host_port).collect(),
    }
}

/// Refuses early if the application's state does not allow this at all.
///
/// The state machine would refuse anyway, but only after Ephemeral had insisted
/// on a working container runtime. Somebody asking to stop an application that
/// was never started should be told that, not told to start Docker.
fn ensure_allowed(manifest: &AppManifest, event: LifecycleEvent) -> Result<()> {
    if manifest.lifecycle.can_apply(event, Actor::User) {
        return Ok(());
    }

    bail!(
        "{}",
        explain_refusal(event, manifest.lifecycle.state(), &manifest.id)
    )
}

/// The Docker runtime, or a refusal that says what to do about it.
fn usable_runtime(workspace: &Workspace) -> Result<DockerRuntime> {
    // Asked before the daemon is, and in that order deliberately: whether
    // Ephemeral *may* drive a container runtime is a question about this
    // machine's owner, and answering "Docker is not installed" to somebody who
    // never allowed Ephemeral to use it would be answering a question they did
    // not ask (ADR-0003).
    ephemeral_api::authority::require(workspace.ledger(), &ephemeral_api::authority::RUNTIME)
        .map_err(Error::msg)?;

    let runtime = DockerRuntime::new();
    let availability = runtime.availability();

    if !availability.usable {
        bail!("{}", availability.explanation);
    }

    Ok(runtime)
}

/// Builds the sandbox specification for an application.
///
/// From the manifest's runtime block and the permissions the ledger says were
/// **granted** — not the ones the manifest requests. That distinction is the
/// reason an application cannot widen its own confinement.
fn specification(workspace: &Workspace, manifest: &AppManifest) -> Result<ContainerSpec> {
    let Some(runtime) = &manifest.runtime else {
        bail!(
            "{} has no runtime yet — it is a record of an intent, not an application. \
             Run `ephemeral generate {}` to write and build one.",
            manifest.id,
            manifest.id
        );
    };

    if runtime.kind != RuntimeKind::Docker {
        bail!(
            "{} declares the {} runtime, and only Docker is implemented so far. \
             See docs/roadmap.md.",
            manifest.id,
            runtime.kind
        );
    }

    let Some(image) = &runtime.image else {
        bail!(
            "{} declares a Docker runtime but names no image, so there is nothing to run.",
            manifest.id
        );
    };

    let paths = HostPaths {
        home: home_directory()?,
        data_dir: workspace.layout().app(&manifest.id).data(),
    };

    let spec = ContainerSpec::from_grants(
        manifest.id.clone(),
        image,
        runtime.entrypoint.clone(),
        manifest.resources,
        &granted_permissions(workspace, &manifest.id),
        &paths,
    )?;

    // Access the user allowed that the sandbox will not give effect to. Saying
    // nothing would leave them believing a decision they made is in force.
    for refusal in &spec.refused {
        println!(
            "{} {} — {}",
            output::warn("Not granting"),
            refusal.granted,
            refusal.reason
        );
    }

    Ok(spec)
}

/// Every app permission currently allowed for this application.
fn granted_permissions(workspace: &Workspace, app: &AppId) -> Vec<AppPermission> {
    // Both halves of the model, which is what makes the second half mean
    // anything: a capability reaches the sandbox only if the person allowed
    // *this application* to have it and allowed *Ephemeral* to carry it out.
    // Filtering on the application's grants alone — which is what this did —
    // left a revoked meta-permission as a note in a ledger nothing read
    // (ADR-0003).
    ephemeral_api::authority::grants(workspace.ledger(), app).effective()
}

/// The user's home directory, which `~` in a permission scope means.
fn home_directory() -> Result<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from)
        .context(
            "could not work out your home directory, so a permission written against `~` \
             cannot be resolved to a real path",
        )
}

/// Applies a lifecycle event, saying what went wrong in the user's terms.
fn apply(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    event: LifecycleEvent,
    actor: Actor,
    reason: &str,
) -> Result<()> {
    let before = manifest.lifecycle.state();

    let applied = manifest
        .apply(TransitionRequest::new(event, actor, reason))
        .with_context(|| explain_refusal(event, before, &manifest.id))?;

    workspace.audit_mut().append(
        actor,
        AuditEvent::LifecycleTransition {
            app: manifest.id.clone(),
            from: applied.from,
            to: applied.to,
            event,
            reason: reason.to_owned(),
        },
    );
    Ok(())
}

/// Why a transition was refused, in terms of what the user asked for.
fn explain_refusal(event: LifecycleEvent, state: LifecycleState, app: &AppId) -> String {
    let base = format!(
        "cannot {event} {app}: it is {}",
        state.headline().to_lowercase()
    );

    match (event, state) {
        (LifecycleEvent::Start, LifecycleState::Running) => {
            format!("{base}. It is already running.")
        }
        (LifecycleEvent::Start, _) if !state.is_runnable() => format!(
            "{base}, and only an application that has been built and validated can be started."
        ),
        (LifecycleEvent::Pause, _) => format!("{base}. Only a running application can be paused."),
        (LifecycleEvent::Resume, _) => {
            format!("{base}. Only a paused application can be resumed.")
        }
        (LifecycleEvent::Stop, _) => format!("{base}. There is nothing running to stop."),
        _ => base,
    }
}

/// One mount, in both the names it has.
///
/// A granted directory has two paths: the one on this machine, which is the one
/// the person granted and recognises, and the one inside the sandbox, which is
/// the only one the application can open. Reporting the first alone is what
/// this used to do, and it reads as an instruction — the obvious next move is to
/// pass that path as an argument, and the application then fails on a file it
/// cannot possibly see. Both, always.
fn describe_mount(mount: &ephemeral_runtime::Mount) -> String {
    format!(
        "Can {} {}, which it sees as {}",
        if mount.writable {
            "read and write"
        } else {
            "read"
        },
        mount.host_path.display(),
        mount.container_path
    )
}

/// How to write the paths this application is given, if it is given any.
///
/// Built from a mount it actually has rather than stated in the abstract: a
/// person about to type a command needs the prefix in front of them, not a rule
/// about prefixes.
fn argument_hint(spec: &ContainerSpec) -> Option<String> {
    let first = spec.host_mounts().next()?;

    Some(format!(
        "Paths you pass to it are the ones it sees: {}/… , not {}/… . \
         Its own storage is {}.",
        first.container_path,
        first.host_path.display(),
        ephemeral_runtime::spec::DATA_MOUNT
    ))
}

/// Tells the user what they just started, and what it can reach.
fn report_started(manifest: &AppManifest, spec: &ContainerSpec, container: Option<&str>) {
    println!(
        "{} {} is {}.",
        output::good("Started."),
        manifest.id,
        output::state(manifest.lifecycle.state())
    );

    if let Some(id) = container {
        println!(
            "{}",
            output::dim(&format!("Container {}", &id[..id.len().min(12)]))
        );
    }

    if spec.is_isolated() {
        println!(
            "{}",
            output::dim("It can see nothing of yours: no files, no network, no open ports.")
        );
        return;
    }

    for mount in spec.host_mounts() {
        println!("{}", output::dim(&describe_mount(mount)));
    }

    if let Some(hint) = argument_hint(spec) {
        println!("{}", output::dim(&hint));
    }

    println!(
        "{}",
        output::dim(&format!("Has {}", spec.egress.describe()))
    );

    for port in &spec.ports {
        println!(
            "{}",
            output::dim(&format!(
                "Listening on http://{}:{}",
                port.host_address(),
                port.host_port
            ))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{
        Principal,
        lifecycle::TransitionContext,
        manifest::{AppInterface, RuntimeSpec},
        permission::{PathScope, Permission},
    };

    fn workspace(root: &Path) -> Workspace {
        Workspace::open(root).unwrap()
    }

    fn app() -> AppId {
        AppId::parse("csv-comparator").unwrap()
    }

    fn built() -> AppManifest {
        let mut manifest = AppManifest::requested(app(), "CSV comparator");
        manifest.runtime = Some(RuntimeSpec::docker_job(
            "python:3.12-slim",
            vec!["python".to_owned(), "compare.py".to_owned()],
        ));
        manifest
    }

    /// The sandbox is built from the ledger, so an application with an empty
    /// ledger gets nothing — whatever its manifest asks for.
    #[test]
    fn a_specification_is_built_from_granted_permissions_only() {
        let home = tempfile::tempdir().unwrap();
        let workspace = workspace(home.path());

        let spec = specification(&workspace, &built()).unwrap();

        assert!(spec.is_isolated());
        assert_eq!(spec.image, "python:3.12-slim");
        assert_eq!(spec.entrypoint, vec!["python", "compare.py"]);
    }

    /// Granting one reaches the sandbox; the point of the previous test is that
    /// nothing else does.
    ///
    /// *Both halves* are granted here, and that is not ceremony: an application
    /// permission on its own is inert, because Ephemeral must also be allowed
    /// to carry it out. The test below is the one that says so.
    #[test]
    fn a_granted_permission_reaches_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());
        let reading = AppPermission::read(PathScope::parse("~/Downloads/**").unwrap());

        workspace
            .ledger_mut()
            .allow(
                Principal::app(app()),
                Permission::App(reading.clone()),
                Actor::User,
                "to compare them",
            )
            .unwrap();
        workspace
            .ledger_mut()
            .allow(
                Principal::Ephemeral,
                Permission::Meta(reading.required_meta()),
                Actor::User,
                "Ephemeral may read what it mounts",
            )
            .unwrap();

        let spec = specification(&workspace, &built()).unwrap();

        assert!(!spec.is_isolated());
        assert_eq!(spec.host_mounts().count(), 1);
        assert!(!spec.host_mounts().next().unwrap().writable);
    }

    /// The rule ADR-0003 states and nothing enforced until now: an application
    /// permission is necessary and not sufficient. Ephemeral has to be allowed
    /// to carry it out, so revoking its authority empties the sandbox of every
    /// application at once rather than leaving a note in a ledger nothing read.
    #[test]
    fn a_permission_ephemeral_may_not_carry_out_never_reaches_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());
        let reading = AppPermission::read(PathScope::parse("~/Downloads/**").unwrap());

        workspace
            .ledger_mut()
            .allow(
                Principal::app(app()),
                Permission::App(reading.clone()),
                Actor::User,
                "to compare them",
            )
            .unwrap();

        assert!(
            specification(&workspace, &built()).unwrap().is_isolated(),
            "the application was allowed; Ephemeral was not"
        );

        workspace
            .ledger_mut()
            .allow(
                Principal::Ephemeral,
                Permission::Meta(reading.required_meta()),
                Actor::User,
                "Ephemeral may read what it mounts",
            )
            .unwrap();
        assert_eq!(
            specification(&workspace, &built())
                .unwrap()
                .host_mounts()
                .count(),
            1,
            "and with both halves it works"
        );

        workspace
            .ledger_mut()
            .revoke(
                &Principal::Ephemeral,
                &Permission::Meta(reading.required_meta()),
                Actor::User,
            )
            .unwrap();
        assert!(
            specification(&workspace, &built()).unwrap().is_isolated(),
            "taking Ephemeral's authority away has to reach the sandbox too"
        );
    }

    /// A sandbox is built once, at start, so a revocation that only changed the
    /// ledger would be a promise about the next run while the present one
    /// carried on with what was just taken away. This is which applications a
    /// revocation has to reach; stopping them is the runtime's part.
    #[test]
    fn a_revocation_reaches_exactly_what_is_running_on_it() {
        let mut running = built();
        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Ephemeral),
            (LifecycleEvent::Start, Actor::User),
            (LifecycleEvent::Started, Actor::Runtime),
        ] {
            running
                .apply(TransitionRequest::new(event, actor, "up and running"))
                .expect("the route to running");
        }

        let idle = built();
        let other = AppId::parse("word-counter").unwrap();

        assert!(
            reaches(&Principal::app(app()), &running),
            "its own permission was taken away while it was using it"
        );
        assert!(
            !reaches(&Principal::app(other.clone()), &running),
            "another application losing something is not this one's business"
        );
        assert!(
            reaches(&Principal::Ephemeral, &running),
            "Ephemeral's authority is what carries every application's out"
        );
        assert!(
            !reaches(&Principal::Ephemeral, &idle),
            "nothing is running, so there is nothing to stop"
        );
        assert!(!reaches(&Principal::app(app()), &idle));
    }

    /// Whether Ephemeral may drive a container runtime is a question about this
    /// machine's owner, and it is asked before Docker is. Answering "Docker is
    /// not installed" to somebody who never allowed Ephemeral to use it answers
    /// a question they did not ask.
    #[test]
    fn driving_a_container_runtime_needs_permission_before_it_needs_a_daemon() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());

        let error = usable_runtime(&workspace)
            .expect_err("nothing has been granted")
            .to_string();
        assert!(error.contains("has not been allowed"), "{error}");
        assert!(
            error.contains("ephemeral grant ephemeral docker"),
            "the refusal has to name the way forward: {error}"
        );

        workspace
            .ledger_mut()
            .allow(
                Principal::Ephemeral,
                Permission::Meta(ephemeral_core::MetaPermission::UseDocker),
                Actor::User,
                "to run applications",
            )
            .unwrap();

        // Past the permission and into the daemon, which is as far as a machine
        // without Docker can go — and the point: the answer is now about Docker.
        if let Err(error) = usable_runtime(&workspace) {
            let message = error.to_string();
            assert!(
                !message.contains("has not been allowed"),
                "permission was granted, so this must be about the runtime: {message}"
            );
        }
    }

    /// A meta-permission is Ephemeral's own authority. It must not be readable
    /// as authority for a generated application, here or anywhere else.
    #[test]
    fn a_meta_permission_never_reaches_an_application_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());

        workspace
            .ledger_mut()
            .allow(
                Principal::Ephemeral,
                Permission::Meta(ephemeral_core::MetaPermission::UseDocker),
                Actor::User,
                "to run applications",
            )
            .unwrap();

        assert!(granted_permissions(&workspace, &app()).is_empty());
    }

    /// A revoked grant stops reaching the sandbox, which is what makes
    /// revocation mean something.
    #[test]
    fn a_revoked_grant_no_longer_reaches_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());
        let permission = Permission::App(AppPermission::read(
            PathScope::parse("~/Downloads/**").unwrap(),
        ));

        workspace
            .ledger_mut()
            .allow(
                Principal::app(app()),
                permission.clone(),
                Actor::User,
                "to compare them",
            )
            .unwrap();
        workspace
            .ledger_mut()
            .revoke(&Principal::app(app()), &permission, Actor::User)
            .unwrap();

        assert!(granted_permissions(&workspace, &app()).is_empty());
        assert!(specification(&workspace, &built()).unwrap().is_isolated());
    }

    /// A granted directory has two names and the application only answers to
    /// one of them. Reporting the one on this machine alone reads as an
    /// instruction — and following it fails on a file the application cannot
    /// possibly see, which is exactly what happened the first time somebody ran
    /// a generated application by hand.
    #[test]
    fn a_mount_is_reported_by_both_of_its_names() {
        let mount = ephemeral_runtime::Mount::read_only("/srv/listings", "/mnt/srv-listings");
        let described = describe_mount(&mount);

        assert!(described.contains("/srv/listings"), "{described}");
        assert!(described.contains("/mnt/srv-listings"), "{described}");
        assert!(described.starts_with("Can read"), "{described}");

        let writable = ephemeral_runtime::Mount::writable("/srv/out", "/mnt/srv-out");
        assert!(describe_mount(&writable).starts_with("Can read and write"));
    }

    /// The hint is built from a mount the application actually has: somebody
    /// about to type a command needs the prefix in front of them, not a rule
    /// about prefixes.
    #[test]
    fn the_paths_to_pass_are_named_when_there_are_any() {
        let mut spec = ContainerSpec::minimal(app(), "python:3.12-slim", vec!["python".to_owned()]);
        assert_eq!(
            argument_hint(&spec),
            None,
            "an application that can see nothing of yours needs no advice about paths"
        );

        spec.mounts.push(ephemeral_runtime::Mount::read_only(
            "/srv/listings",
            "/mnt/srv-listings",
        ));
        let hint = argument_hint(&spec).expect("a hint");

        assert!(hint.contains("/mnt/srv-listings/"), "{hint}");
        assert!(hint.contains("/srv/listings/"), "{hint}");
        assert!(
            hint.contains("/data"),
            "its own storage is worth naming: {hint}"
        );
    }

    /// The output is withheld from an application that never had a container,
    /// and shown for one whose container outlived it — which is every state a
    /// run can end in, crashes included. It used to be the other way round for
    /// a crash: the traceback explaining the failure was the one thing not
    /// printed.
    #[test]
    fn a_crashed_application_still_has_its_last_words() {
        let mut crashed = built();
        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Ephemeral),
            (LifecycleEvent::Start, Actor::User),
            (LifecycleEvent::Started, Actor::Runtime),
            (LifecycleEvent::RuntimeCrashed, Actor::Runtime),
        ] {
            crashed
                .apply(TransitionRequest::new(
                    event,
                    actor,
                    "generated, run, crashed",
                ))
                .expect("the route from a description to a crash");
        }

        assert_eq!(crashed.lifecycle.state(), LifecycleState::RuntimeFailed);
        assert!(
            has_output(&crashed),
            "a crash is when its output matters most"
        );
        assert!(has_output(&built()), "a built application may have run");
        assert!(
            !has_output(&AppManifest::requested(app(), "CSV comparator")),
            "an application that was never built never had a container"
        );
    }

    /// An application that has not been generated yet says so, rather than
    /// failing somewhere inside Docker.
    #[test]
    fn an_app_with_no_runtime_explains_itself() {
        let home = tempfile::tempdir().unwrap();
        let workspace = workspace(home.path());

        let error = specification(&workspace, &AppManifest::requested(app(), "CSV comparator"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("no runtime yet"), "{error}");
        assert!(
            error.contains("ephemeral generate"),
            "an application that has not been built yet is told how to build it: {error}"
        );
    }

    #[test]
    fn a_runtime_this_version_cannot_provide_says_which() {
        let home = tempfile::tempdir().unwrap();
        let workspace = workspace(home.path());

        let mut manifest = built();
        manifest.runtime = Some(RuntimeSpec {
            kind: RuntimeKind::Native,
            image: None,
            version: None,
            entrypoint: vec![],
            interface: AppInterface::CommandLine,
            port: None,
        });

        let error = specification(&workspace, &manifest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("native"), "{error}");
    }

    /// A manifest that has just started running, with the given ceiling.
    ///
    /// The tests move `now` forward rather than backdating the start: the clock
    /// is a parameter of `overran` precisely so that testing it needs no
    /// waiting and no fabricated history.
    fn running(limit: Option<RetentionPeriod>) -> AppManifest {
        let mut manifest = built();
        manifest.resources.max_runtime = limit;

        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Runtime),
            (LifecycleEvent::Start, Actor::User),
            (LifecycleEvent::Started, Actor::Runtime),
        ] {
            manifest
                .apply(TransitionRequest::new(event, actor, "test"))
                .unwrap();
        }

        manifest
    }

    /// The time a limit is measured from is when Ephemeral started it, which is
    /// what the history records.
    #[test]
    fn a_running_application_knows_when_it_started() {
        let manifest = running(None);
        let since = running_since(&manifest).expect("a started app has a start time");

        assert!((ephemeral_core::now() - since).num_seconds() < 5);
        assert_eq!(running_since(&built()), None, "it was never started");
    }

    /// A ceiling that is not exceeded must not stop anything, and an
    /// application with no ceiling must never be stopped for time.
    #[test]
    fn an_application_within_its_limit_is_left_running() {
        let manifest = running(Some(RetentionPeriod::seconds(900)));
        assert_eq!(overran(&manifest, ephemeral_core::now()), None);

        // No ceiling means no ceiling, however long it runs.
        let forever = running(None);
        let much_later = ephemeral_core::now() + chrono::Duration::days(365);
        assert_eq!(overran(&forever, much_later), None);
    }

    /// Past the ceiling, by how much.
    #[test]
    fn an_application_past_its_limit_is_reported_with_the_overrun() {
        let manifest = running(Some(RetentionPeriod::seconds(900)));
        let later = ephemeral_core::now() + chrono::Duration::seconds(1000);

        assert_eq!(overran(&manifest, later), Some(100));
    }

    /// An application that was never started has no clock to be past.
    #[test]
    fn an_application_that_never_started_cannot_overrun() {
        let mut manifest = built();
        manifest.resources.max_runtime = Some(RetentionPeriod::seconds(1));

        let later = ephemeral_core::now() + chrono::Duration::days(1);
        assert_eq!(overran(&manifest, later), None);
    }

    fn observed(
        state: ephemeral_runtime::ContainerState,
        exit: Option<i64>,
        health: Option<&str>,
    ) -> ContainerStatus {
        ContainerStatus {
            app: app(),
            state,
            container_id: Some("abc123".to_owned()),
            exit_code: exit,
            health: health.map(ToOwned::to_owned),
        }
    }

    /// Every correction the observer can propose must be one the state machine
    /// accepts *from the actor proposing it*. Only the runtime may report a
    /// fact; only a person or Ephemeral may express an intention. A mismatch
    /// here would surface as a command that fails whenever it is most needed.
    #[test]
    fn every_implied_correction_is_legal_for_the_actor_that_raises_it() {
        use ephemeral_runtime::ContainerState as C;

        let cases = [
            (LifecycleState::Running, observed(C::Absent, None, None)),
            (LifecycleState::Paused, observed(C::Absent, None, None)),
            (LifecycleState::Unhealthy, observed(C::Absent, None, None)),
            (LifecycleState::Running, observed(C::Exited, Some(0), None)),
            (
                LifecycleState::Unhealthy,
                observed(C::Exited, Some(0), None),
            ),
            (
                LifecycleState::Running,
                observed(C::Exited, Some(137), None),
            ),
            (LifecycleState::Running, observed(C::Dead, None, None)),
            (
                LifecycleState::Running,
                observed(C::Running, None, Some("unhealthy")),
            ),
            (
                LifecycleState::Unhealthy,
                observed(C::Running, None, Some("healthy")),
            ),
            (LifecycleState::Running, observed(C::Paused, None, None)),
            (LifecycleState::Paused, observed(C::Running, None, None)),
        ];

        for (recorded, status) in cases {
            let steps = implied_events(recorded, &status);
            assert!(
                !steps.is_empty(),
                "{recorded} vs {} should imply a correction",
                status.state
            );

            // Replayed through the machine itself rather than asserting the
            // resulting states by hand.
            let mut state = recorded;
            for (event, actor, _) in steps {
                assert!(event.permits(actor), "{actor:?} may not raise {event}");
                state = state
                    .next(event, &TransitionContext::default())
                    .unwrap_or_else(|error| panic!("{recorded} --{event}--> ? : {error}"));
            }
            assert_ne!(state, recorded, "a correction should change something");
        }
    }

    /// A container doing exactly what the record says must not be corrected.
    #[test]
    fn agreement_implies_no_correction() {
        use ephemeral_runtime::ContainerState as C;

        assert!(
            implied_events(LifecycleState::Running, &observed(C::Running, None, None)).is_empty()
        );
        assert!(
            implied_events(LifecycleState::Paused, &observed(C::Paused, None, None)).is_empty()
        );
        assert!(
            implied_events(
                LifecycleState::Running,
                &observed(C::Running, None, Some("healthy"))
            )
            .is_empty()
        );
    }

    /// An application that finished its work has not failed, and must not be
    /// reported as having crashed.
    #[test]
    fn a_clean_exit_is_finishing_rather_than_crashing() {
        use ephemeral_runtime::ContainerState as C;

        let steps = implied_events(LifecycleState::Running, &observed(C::Exited, Some(0), None));
        assert!(
            steps
                .iter()
                .all(|(event, _, _)| *event != LifecycleEvent::RuntimeCrashed),
            "{steps:?}"
        );

        let failed = implied_events(LifecycleState::Running, &observed(C::Exited, Some(1), None));
        assert_eq!(failed[0].0, LifecycleEvent::RuntimeCrashed);
        assert!(failed[0].2.contains("code 1"), "{failed:?}");
    }

    fn held(
        name: &str,
        app: Option<&str>,
        state: ephemeral_runtime::ContainerState,
    ) -> ManagedContainer {
        ManagedContainer {
            name: name.to_owned(),
            app: app.map(|id| AppId::parse(id).unwrap()),
            state,
        }
    }

    /// An application inside its ceiling must not be stopped, and an
    /// unreadable directory is a reason to say nothing rather than to stop
    /// something.
    #[test]
    fn an_application_within_its_disk_ceiling_is_left_alone() {
        let data = tempfile::tempdir().unwrap();
        let manifest = running(None);

        assert_eq!(storage_used_mib(data.path()), Some(0));
        assert_eq!(overfilled(&manifest, data.path()), None);

        // A directory that is not there cannot be measured, and an application
        // must not be stopped because Ephemeral could not look.
        assert_eq!(
            overfilled(&manifest, &data.path().join("missing")),
            None,
            "an unreadable directory must not stop an application"
        );
    }

    /// Past the ceiling, by how much. The ceiling was in every manifest and
    /// enforced by nothing until this existed.
    #[test]
    fn an_application_past_its_disk_ceiling_is_reported() {
        let data = tempfile::tempdir().unwrap();

        let mut manifest = running(None);
        manifest.resources.storage_mib = 1;

        // Two mebibytes, in a nested directory so the walk is exercised.
        let nested = data.path().join("output/rows");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("big.csv"), vec![b'x'; 2 * 1024 * 1024]).unwrap();

        assert_eq!(storage_used_mib(data.path()), Some(2));
        assert_eq!(overfilled(&manifest, data.path()), Some(1));
    }

    /// A container belonging to an application that is genuinely running must
    /// never be reaped. This is the direction that destroys somebody's work.
    #[test]
    fn a_container_the_manifest_accounts_for_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());

        // Driven through the machine rather than assigned, so the test breaks
        // if the route to a running application ever changes.
        let mut manifest = built();
        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Runtime),
            (LifecycleEvent::Start, Actor::User),
            (LifecycleEvent::Started, Actor::Runtime),
        ] {
            manifest
                .apply(TransitionRequest::new(event, actor, "test"))
                .unwrap_or_else(|error| panic!("{event}: {error}"));
        }
        assert!(manifest.lifecycle.state().requires_runtime());
        workspace.apps_mut().save(&manifest).unwrap();

        let containers = vec![held(
            "ephemeral-csv-comparator",
            Some("csv-comparator"),
            ephemeral_runtime::ContainerState::Running,
        )];

        assert!(classify(&workspace, &containers).is_empty());
    }

    /// A container whose application was purged out from under it.
    #[test]
    fn a_container_whose_application_is_gone_is_an_orphan() {
        let home = tempfile::tempdir().unwrap();
        let workspace = workspace(home.path());

        let containers = vec![held(
            "ephemeral-csv-comparator",
            Some("csv-comparator"),
            ephemeral_runtime::ContainerState::Exited,
        )];

        let found = classify(&workspace, &containers);
        assert_eq!(found.len(), 1);
        assert!(found[0].reason.contains("no longer exists"), "{found:?}");
    }

    /// The manifest is what the user is shown, so when the two disagree it is
    /// the container that is wrong.
    #[test]
    fn a_container_the_lifecycle_says_should_not_exist_is_an_orphan() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());

        // Requested: created, never built, certainly never started.
        let manifest = AppManifest::requested(app(), "CSV comparator");
        assert!(!manifest.lifecycle.state().requires_runtime());
        workspace.apps_mut().save(&manifest).unwrap();

        let containers = vec![held(
            "ephemeral-csv-comparator",
            Some("csv-comparator"),
            ephemeral_runtime::ContainerState::Running,
        )];

        let found = classify(&workspace, &containers);
        assert_eq!(found.len(), 1);
        assert!(
            found[0].reason.contains("does not hold a container"),
            "{found:?}"
        );
    }

    /// A labelled container with no readable application id is Ephemeral's
    /// mess either way, so it is reported rather than ignored.
    #[test]
    fn a_labelled_container_naming_no_application_is_an_orphan() {
        let home = tempfile::tempdir().unwrap();
        let workspace = workspace(home.path());

        let containers = vec![held(
            "ephemeral-something",
            None,
            ephemeral_runtime::ContainerState::Dead,
        )];

        assert_eq!(classify(&workspace, &containers).len(), 1);
    }

    /// A refusal has to say what the user asked for and why it did not happen.
    /// "Invalid state transition" is not that.
    #[test]
    fn refusals_are_explained_in_the_users_terms() {
        let already = explain_refusal(LifecycleEvent::Start, LifecycleState::Running, &app());
        assert!(already.contains("already running"), "{already}");

        let unbuilt = explain_refusal(LifecycleEvent::Start, LifecycleState::Requested, &app());
        assert!(unbuilt.contains("built and validated"), "{unbuilt}");

        let not_paused = explain_refusal(LifecycleEvent::Resume, LifecycleState::Running, &app());
        assert!(not_paused.contains("paused"), "{not_paused}");
    }
}
