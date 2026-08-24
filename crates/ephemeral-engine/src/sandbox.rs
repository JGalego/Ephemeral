//! What confines an application, decided from the ledger.
//!
//! The specification is built from what the ledger says was **granted** — never
//! from what the manifest requests — which is the reason an application cannot
//! widen its own confinement by asking for more. Both halves of the permission
//! model apply ([ADR-0003]): a capability reaches the sandbox only if the
//! person allowed this application to have it *and* allowed Ephemeral to carry
//! it out.
//!
//! [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md

use anyhow::{Context as _, Error, Result, bail};
use ephemeral_core::{
    Actor, AppId, AppManifest,
    audit::AuditEvent,
    lifecycle::{LifecycleEvent, LifecycleState, TransitionRequest},
    manifest::RuntimeKind,
    permission::AppPermission,
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::{ContainerSpec, HostPaths, Mount, Runtime as _, docker::DockerRuntime};

/// A sandbox specification, and what a person should be told about it.
#[derive(Debug, Clone)]
pub struct Confinement {
    /// What the runtime will be asked for.
    pub spec: ContainerSpec,

    /// Access that was granted and will not be given effect, with the reason.
    ///
    /// Saying nothing would leave somebody believing a decision they made is in
    /// force. The commonest case is an egress allow-list, which Docker cannot
    /// express and Ephemeral refuses to approximate.
    pub refused: Vec<String>,

    /// Capabilities the person allowed that Ephemeral itself may not carry out.
    pub inert: Option<String>,
}

/// Builds the sandbox specification for an application.
///
/// # Errors
///
/// If the application has no runtime yet, declares one this version cannot
/// provide, names no image, or asks for something that cannot be resolved to a
/// real path on this machine.
pub fn specification(workspace: &Workspace, manifest: &AppManifest) -> Result<Confinement> {
    let Some(runtime) = &manifest.runtime else {
        bail!(
            "{} has no runtime yet — it is a record of an intent, not an application. \
             Run `ephemeral generate {}` to write and build one.",
            manifest.id,
            manifest.id
        );
    };

    if runtime.kind != RuntimeKind::Docker {
        // This function builds a *container* specification, so reaching it with
        // anything else is a caller that took the wrong branch rather than a
        // runtime that is missing. Saying which is which matters: one of these
        // is a bug in Ephemeral and the other is a decision.
        let where_it_stands = match runtime.kind {
            RuntimeKind::Wasm => {
                "this builds a container specification, and a WebAssembly application \
                 does not have one — `container::run_once` is the path for it"
            }
            RuntimeKind::Native => "nothing implements the native runtime, deliberately (ADR-0015)",
            _ => "only Docker and WebAssembly are implemented so far",
        };
        bail!(
            "{} declares the {} runtime: {where_it_stands}. See docs/roadmap.md.",
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

    let refused = spec
        .refused
        .iter()
        .map(|refusal| format!("Not granting {} — {}", refusal.granted, refusal.reason))
        .collect();

    Ok(Confinement {
        inert: ephemeral_api::authority::grants(workspace.ledger(), &manifest.id).explain_inert(),
        refused,
        spec,
    })
}

/// Every app permission currently allowed *and* carryable for this application.
///
/// Both halves of the model, which is what makes the second half mean anything:
/// filtering on the application's grants alone left a revoked meta-permission
/// as a note in a ledger nothing read.
#[must_use]
pub fn granted_permissions(workspace: &Workspace, app: &AppId) -> Vec<AppPermission> {
    ephemeral_api::authority::grants(workspace.ledger(), app).effective()
}

/// The user's home directory, which `~` in a permission scope means.
///
/// # Errors
///
/// If the environment does not say where it is, since a scope written against
/// `~` cannot then be resolved to anything real.
pub fn home_directory() -> Result<std::path::PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from)
        .context(
            "could not work out your home directory, so a permission written against `~` \
             cannot be resolved to a real path",
        )
}

/// A container runtime Ephemeral is allowed to drive, and that answers.
///
/// # Errors
///
/// [`ephemeral_api::authority::require`]'s refusal when Ephemeral has not been
/// allowed to drive one, and the runtime's own explanation when there is none
/// to drive. In that order deliberately: whether Ephemeral *may* use a
/// container runtime is a question about this machine's owner, and answering
/// "Docker is not installed" to somebody who never allowed Ephemeral to use it
/// answers a question they did not ask.
pub fn usable_runtime(workspace: &Workspace) -> Result<DockerRuntime> {
    ephemeral_api::authority::require(workspace.ledger(), &ephemeral_api::authority::RUNTIME)
        .map_err(Error::msg)?;

    let runtime = DockerRuntime::new();
    let availability = runtime.availability();

    if !availability.usable {
        bail!("{}", availability.explanation);
    }

    Ok(runtime)
}

/// Refuses early if the application's state does not allow this at all.
///
/// The state machine would refuse anyway, but only after Ephemeral had insisted
/// on a working container runtime. Somebody asking to stop an application that
/// was never started should be told that, not told to start Docker.
///
/// # Errors
///
/// An explanation in terms of what the person asked for.
pub fn ensure_allowed(manifest: &AppManifest, event: LifecycleEvent) -> Result<()> {
    if manifest.lifecycle.can_apply(event, Actor::User) {
        return Ok(());
    }

    bail!(
        "{}",
        explain_refusal(event, manifest.lifecycle.state(), &manifest.id)
    )
}

/// Why a transition was refused, in terms of what the user asked for.
#[must_use]
pub fn explain_refusal(event: LifecycleEvent, state: LifecycleState, app: &AppId) -> String {
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

/// Applies a lifecycle event and records it, saying what went wrong in the
/// user's terms.
///
/// # Errors
///
/// [`explain_refusal`]'s wording when the state machine will not have it.
pub fn apply(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    event: LifecycleEvent,
    actor: Actor,
    reason: &str,
) -> Result<()> {
    let applied = manifest
        .apply(TransitionRequest::new(event, actor, reason))
        .with_context(|| explain_refusal(event, manifest.lifecycle.state(), &manifest.id))?;

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

/// Saves a manifest and the workspace together.
///
/// # Errors
///
/// Whatever the store says. Both are saved or the caller hears about it: a
/// manifest that moved without its audit entry is a record that lies.
pub fn save(workspace: &mut Workspace, manifest: &AppManifest) -> Result<()> {
    workspace.apps_mut().save(manifest)?;
    workspace.save()?;
    Ok(())
}

/// The record of what a sandbox was actually given.
#[must_use]
pub fn sandbox_created(runtime: &str, spec: &ContainerSpec) -> AuditEvent {
    AuditEvent::SandboxCreated {
        app: spec.app.clone(),
        runtime: runtime.to_owned(),
        // A runtime with no images leaves this empty, and an empty string in
        // an audit record reads as an image nobody wrote down rather than as
        // one that never existed.
        image: (!spec.image.is_empty()).then(|| spec.image.clone()),
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

/// One mount, in both the names it has.
///
/// A granted directory has two paths: the one on this machine, which is the one
/// the person granted and recognises, and the one inside the sandbox, which is
/// the only one the application can open. Reporting the first alone reads as an
/// instruction — and following it fails on a file the application cannot
/// possibly see.
#[must_use]
pub fn describe_mount(mount: &Mount) -> String {
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
#[must_use]
pub fn argument_hint(spec: &ContainerSpec) -> Option<String> {
    let first = spec.host_mounts().next()?;

    Some(format!(
        "Paths you pass to it are the ones it sees: {}/… , not {}/… . Its own storage is {}.",
        first.container_path,
        first.host_path.display(),
        ephemeral_runtime::spec::DATA_MOUNT
    ))
}

/// What an application can reach, said the way both clients say it.
#[must_use]
pub fn confinement(spec: &ContainerSpec) -> Vec<String> {
    if spec.is_isolated() {
        return vec![
            "It can see nothing of yours: no files, no network, no open ports.".to_owned(),
        ];
    }

    let mut lines: Vec<String> = spec.host_mounts().map(describe_mount).collect();
    lines.extend(argument_hint(spec));
    lines.push(format!("Has {}", spec.egress.describe()));
    lines.extend(spec.ports.iter().map(|port| {
        format!(
            "Listening on http://{}:{}",
            port.host_address(),
            port.host_port
        )
    }));

    lines
}
