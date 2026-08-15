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

use anyhow::{Context as _, Result, bail};
use ephemeral_core::{
    Actor, AppId, AppManifest, Principal,
    audit::{AuditEvent, AuditLog},
    lifecycle::{LifecycleEvent, LifecycleState, TransitionRequest},
    manifest::RuntimeKind,
    permission::{AppPermission, Permission},
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::{
    ContainerSpec, HostPaths, ManagedContainer, Runtime as _, RuntimeError, Secrets,
    docker::DockerRuntime,
};

use crate::output;

/// Starts an application.
pub(crate) fn run(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    ensure_allowed(&manifest, LifecycleEvent::Start)?;
    let spec = specification(&workspace, &manifest)?;
    let runtime = usable_runtime()?;

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
    let runtime = usable_runtime()?;

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

/// Suspends a running application without losing its state.
pub(crate) fn pause(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    // What the application is doing is a better answer than what Docker is
    // doing: "it is not running" beats "Docker is not responding" when both
    // are true.
    ensure_allowed(&manifest, LifecycleEvent::Pause)?;
    let runtime = usable_runtime()?;

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
    let runtime = usable_runtime()?;

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
    let runtime = usable_runtime()?;

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
fn usable_runtime() -> Result<DockerRuntime> {
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
             Generating one arrives in Phase 2; see docs/roadmap.md.",
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
    workspace
        .ledger()
        .active_grants(&Principal::app(app.clone()))
        .into_iter()
        .filter(|grant| grant.decision.is_allowed())
        .filter_map(|grant| match &grant.permission {
            Permission::App(permission) => Some(permission.clone()),
            // A meta-permission is Ephemeral's own authority and never an
            // application's. It cannot reach the sandbox even by accident.
            Permission::Meta(_) => None,
        })
        .collect()
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
        println!(
            "{}",
            output::dim(&format!(
                "Can {} {}",
                if mount.writable {
                    "read and write"
                } else {
                    "read"
                },
                mount.host_path.display()
            ))
        );
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

/// Reports a runtime failure as a lifecycle fact, without a running container.
///
/// Kept here so that `RuntimeError` never has to be understood by the audit
/// module, which knows about applications rather than about Docker.
#[allow(dead_code, reason = "used by the supervisor in Phase 2")]
fn note_crash(audit: &mut AuditLog, app: &AppId, error: &RuntimeError) {
    audit.append(
        Actor::Runtime,
        AuditEvent::LifecycleTransition {
            app: app.clone(),
            from: LifecycleState::Running,
            to: LifecycleState::RuntimeFailed,
            event: LifecycleEvent::RuntimeCrashed,
            reason: error.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{
        manifest::{AppInterface, RuntimeSpec},
        permission::PathScope,
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
    #[test]
    fn a_granted_permission_reaches_the_sandbox() {
        let home = tempfile::tempdir().unwrap();
        let mut workspace = workspace(home.path());

        workspace
            .ledger_mut()
            .allow(
                Principal::app(app()),
                Permission::App(AppPermission::read(
                    PathScope::parse("~/Downloads/**").unwrap(),
                )),
                Actor::User,
                "to compare them",
            )
            .unwrap();

        let spec = specification(&workspace, &built()).unwrap();

        assert!(!spec.is_isolated());
        assert_eq!(spec.host_mounts().count(), 1);
        assert!(!spec.host_mounts().next().unwrap().writable);
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
        assert!(error.contains("Phase 2"), "{error}");
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
