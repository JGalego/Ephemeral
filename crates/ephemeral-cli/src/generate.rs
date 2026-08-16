//! `ephemeral generate` — turn a recorded intent into an application.
//!
//! This is where the two untrusted things meet: a model writes the code, and
//! the runtime builds and tests it. Neither is trusted, and the arrangement
//! reflects that.
//!
//! - The model's output is validated before a byte of it reaches the disk, and
//!   refused rather than repaired when it is wrong.
//! - Source is written under the application's own directory, and a path that
//!   would escape it stops the run.
//! - The permissions the plan asks for are recorded as **requests**. Nothing
//!   here grants anything; `ephemeral grant` is still a person's decision.
//! - Every lifecycle transition corresponds to something that actually
//!   happened. `ValidationPassed` is emitted because tests ran and passed, not
//!   because a build finished.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use ephemeral_agent::{
    AgentError, AgentProvider, Builder, GeneratedApp, MockProvider, Outcome, RealClock, Run,
    SourceFile, build::NeverCancelled, generate as run_loop,
};
use ephemeral_core::{
    Actor, AppManifest,
    audit::AuditEvent,
    lifecycle::{LifecycleEvent, TransitionRequest},
    manifest::{PermissionRationale, RuntimeSpec},
    permission::AppPermissions,
    storage::{AppStore as _, Workspace},
};
use ephemeral_runtime::{BuildRequest, Runtime as _, docker::DockerRuntime};

use crate::output;

/// The file a build recipe is written to.
const DOCKERFILE: &str = "Dockerfile";

/// The provisional tag a build carries before its version is known.
const BUILDING_TAG: &str = "building";

/// Builds an application from the intent already recorded for it.
pub(crate) fn run(home: &Path, reference: &str, provider_name: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    let regenerating = manifest.runtime.is_some();

    if !manifest
        .lifecycle
        .can_apply(LifecycleEvent::Plan, Actor::Ephemeral)
    {
        bail!(
            "cannot {} {}: it is {}. {}",
            if regenerating {
                "regenerate"
            } else {
                "generate"
            },
            manifest.id,
            manifest.lifecycle.state().headline().to_lowercase(),
            if manifest.lifecycle.state().is_runnable() {
                "Stop it first."
            } else {
                ""
            }
        );
    }

    let provider = provider(provider_name)?;
    provider
        .availability()
        .with_context(|| format!("{provider_name} cannot be used"))?;

    let runtime = DockerRuntime::new();
    let availability = runtime.availability();
    if !availability.usable {
        bail!("{}", availability.explanation);
    }

    let source_dir = workspace.layout().app(&manifest.id).source();
    std::fs::create_dir_all(&source_dir)
        .with_context(|| format!("could not create {}", source_dir.display()))?;

    println!(
        "{} {} with {}",
        output::dim("Generating"),
        manifest.id,
        provider.name()
    );

    workspace.audit_mut().append(
        Actor::Ephemeral,
        AuditEvent::GenerationStarted {
            app: manifest.id.clone(),
            provider: provider.name().to_owned(),
        },
    );

    // Planning has begun, and the record says so before anything is asked of a
    // model. A run that dies mid-flight leaves an application in `Planning`,
    // which is a state the machine has.
    step(
        &mut workspace,
        &mut manifest,
        LifecycleEvent::Plan,
        Actor::Ephemeral,
        &format!("planning with {}", provider.name()),
    )?;
    workspace.apps_mut().save(&manifest)?;
    workspace.save()?;

    let builder = DockerBuilder {
        runtime: &runtime,
        source_dir: source_dir.clone(),
        app: manifest.id.clone(),
    };

    let clock = RealClock::start();
    let outcome = run_loop(
        provider.as_ref(),
        &manifest.metadata.purpose,
        &Run {
            budget: &manifest.budget,
            builder: &builder,
            cancellation: &NeverCancelled,
            clock: &clock,
        },
    );

    match outcome {
        Ok(outcome) => finish(
            &mut workspace,
            &mut manifest,
            &outcome,
            provider.name(),
            regenerating,
            &runtime,
        ),
        Err(error) => {
            fail(&mut workspace, &mut manifest, &error)?;
            Err(anyhow::Error::new(error).context(format!("could not generate {}", manifest.id)))
        }
    }
}

/// What happened, as the state machine sees it.
#[derive(Debug, Clone)]
struct Recorded {
    event: LifecycleEvent,
    actor: Actor,
    from: ephemeral_core::LifecycleState,
    to: ephemeral_core::LifecycleState,
    reason: String,
}

/// Moves a manifest from `Planning` to `Ready`, recording what the run produced.
///
/// The interleaving is not cosmetic. A manifest must know what it runs on from
/// the moment it is `Building` — `AppManifest::validate` insists, because an
/// application that is being built and cannot say what it is being built *as*
/// is a record that means nothing. So planning settles the runtime, and only
/// then does generation report having finished.
///
/// Returns the transitions for the caller to audit, and what this version newly
/// asks for compared with the one before it.
///
/// # Errors
///
/// Whatever the state machine says, if a transition is not legal from where the
/// application actually is.
fn apply_success(
    manifest: &mut AppManifest,
    outcome: &Outcome,
    provider: &str,
) -> Result<(Vec<Recorded>, ephemeral_core::PermissionDelta)> {
    let mut recorded = Vec::new();

    let mut apply = |manifest: &mut AppManifest,
                     event: LifecycleEvent,
                     actor: Actor,
                     reason: String|
     -> Result<()> {
        let transition = manifest
            .apply(TransitionRequest::new(event, actor, &reason))
            .with_context(|| {
                format!(
                    "cannot {event} {} while it is {}",
                    manifest.id,
                    manifest.lifecycle.state().headline().to_lowercase()
                )
            })?;

        recorded.push(Recorded {
            event,
            actor,
            from: transition.from,
            to: transition.to,
            reason,
        });
        Ok(())
    };

    apply(
        manifest,
        LifecycleEvent::PlanCompleted,
        Actor::Ephemeral,
        outcome.app.plan.summary.clone(),
    )?;

    // Planning is what settles this, so it is recorded the moment planning
    // finishes and before anything claims to have built it.
    manifest.runtime = Some(RuntimeSpec {
        kind: outcome.app.plan.runtime,
        image: Some(outcome.app.plan.image.clone()),
        version: None,
        entrypoint: outcome.app.entrypoint.clone(),
        interface: outcome.app.plan.interface,
        port: None,
    });

    // Requests, never grants. The manifest records what the application wants;
    // the ledger — which only a person writes to — records what it has.
    manifest.permissions = requested_permissions(&outcome.app);

    // The reasons travel with the requests, because a prompt has to answer
    // "why does it need this?" long after planning produced the answer.
    manifest.rationale = outcome
        .app
        .plan
        .requests
        .iter()
        .map(|request| PermissionRationale {
            permission: request.permission.clone(),
            reason: request.reason.clone(),
        })
        .collect();

    apply(
        manifest,
        LifecycleEvent::GenerationCompleted,
        Actor::Agent,
        format!("{} file(s) written", outcome.files.len()),
    )?;
    apply(
        manifest,
        LifecycleEvent::BuildSucceeded,
        Actor::Runtime,
        match outcome.repairs() {
            0 => "built first time".to_owned(),
            n => format!("built after {n} fix(es)"),
        },
    )?;
    apply(
        manifest,
        LifecycleEvent::ValidationPassed,
        Actor::Runtime,
        format!("its tests ran and passed ({})", outcome.usage.describe()),
    )?;

    let delta = manifest.record_version(
        &outcome.recipe(&manifest.resources.describe()),
        format!("generated with {provider}"),
    );

    Ok((recorded, delta))
}

/// Records everything a successful run produced.
fn finish(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    outcome: &Outcome,
    provider: &str,
    regenerating: bool,
    runtime: &DockerRuntime,
) -> Result<()> {
    let (recorded, delta) = apply_success(manifest, outcome, provider)?;

    // Keep this version's source before anything overwrites it. A digest in the
    // history that nothing can restore is half a promise: it says what the
    // application was and cannot put it back. Failing to keep it is not worth
    // failing a successful generation over, so it is reported and the run
    // stands — but it is reported, rather than leaving somebody to discover it
    // when a rollback has nothing to roll back to.
    if let Some(version) = manifest.current_version()
        && let Err(error) = workspace.apps().keep_version(&manifest.id, &version.digest)
    {
        eprintln!(
            "{} this version's source was not kept, so you will not be able to \
             return to it: {error}",
            output::warn("warning")
        );
    }

    // The image Ephemeral built, not the base image it was built from. Running
    // the base image would run something that has never seen the generated
    // source — which is exactly what happened the first time this was run for
    // real, and no argument-vector test could have caught it.
    if let Some(version) = manifest.current_version() {
        let digest = version.digest.short().to_owned();
        let built = ephemeral_runtime::docker::command::building_tag(&manifest.id);

        let named = runtime
            .name_image(&built, &manifest.id, &digest)
            .with_context(|| format!("could not name the image built for {}", manifest.id))?;

        if let Some(spec) = manifest.runtime.as_mut() {
            spec.image = Some(named);
        }
    }

    // The question ADR-0011 exists to answer. A new version that wants more
    // than the one already approved must not inherit that approval, so anything
    // newly requested has its existing grants withdrawn — the user is asked
    // again rather than assumed to have agreed in advance.
    let withdrawn = if delta.widens() {
        withdraw_widened(workspace, &manifest.id, &delta)
    } else {
        0
    };

    for entry in recorded {
        workspace.audit_mut().append(
            entry.actor,
            AuditEvent::LifecycleTransition {
                app: manifest.id.clone(),
                from: entry.from,
                to: entry.to,
                event: entry.event,
                reason: entry.reason,
            },
        );
    }

    workspace.audit_mut().append(
        Actor::Ephemeral,
        AuditEvent::GenerationFinished {
            app: manifest.id.clone(),
            succeeded: true,
            repairs: u32::try_from(outcome.repairs()).unwrap_or(u32::MAX),
        },
    );

    workspace.apps_mut().save(manifest)?;
    workspace.save()?;

    report(manifest, outcome, &delta);

    if delta.widens() {
        println!();
        println!(
            "{} {}",
            output::warn("This update wants more."),
            delta.describe()
        );
        if withdrawn > 0 {
            println!(
                "{}",
                output::dim(&format!(
                    "{withdrawn} permission(s) you had allowed were withdrawn, because they no \
                     longer cover what it now asks for."
                ))
            );
        }
        println!(
            "{}",
            output::dim(&format!(
                "Run `ephemeral review {}` to decide. Until you do, it has less than it had.",
                manifest.id
            ))
        );
    } else if regenerating {
        println!();
        println!(
            "{}",
            output::dim("This update asks for nothing new, so your existing decisions stand.")
        );
    }

    Ok(())
}

/// Withdraws grants that a widening update would otherwise silently inherit.
///
/// Returns how many were withdrawn. Only the ones the *new* request touches:
/// an update that adds network access does not cost the user the file access
/// they already agreed to.
pub(crate) fn withdraw_widened(
    workspace: &mut Workspace,
    app: &ephemeral_core::AppId,
    delta: &ephemeral_core::PermissionDelta,
) -> usize {
    let subject = ephemeral_core::Principal::app(app.clone());
    let mut withdrawn = 0;

    for permission in &delta.added {
        withdrawn += workspace
            .ledger_mut()
            .revoke(
                &subject,
                &ephemeral_core::permission::Permission::App(permission.clone()),
                Actor::User,
            )
            .unwrap_or(0);
    }

    withdrawn
}

/// Records a failed run, so the manifest says what happened.
fn fail(workspace: &mut Workspace, manifest: &mut AppManifest, error: &AgentError) -> Result<()> {
    // A run that got as far as producing code and failed to build is a
    // different story from one that never planned, and the machine can tell
    // them apart only if the events it is given are the ones that happened.
    let route: &[(LifecycleEvent, Actor)] = match error {
        AgentError::Refused(_) | AgentError::Unreadable { .. } | AgentError::Unavailable { .. } => {
            &[(LifecycleEvent::Block, Actor::Ephemeral)]
        }
        AgentError::Cancelled => &[(LifecycleEvent::Cancel, Actor::User)],
        _ => &[
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildFailed, Actor::Runtime),
        ],
    };

    for (event, actor) in route {
        // Best-effort: the run has already failed, and a state machine refusal
        // here must not replace the real error with a less useful one.
        if step(workspace, manifest, *event, *actor, &error.to_string()).is_err() {
            break;
        }
    }

    workspace.apps_mut().save(manifest)?;
    workspace.save()?;
    Ok(())
}

/// The permissions a generated application asks for.
fn requested_permissions(app: &GeneratedApp) -> AppPermissions {
    let mut permissions = AppPermissions::none();

    for request in &app.plan.requests {
        permissions.request(&request.permission);
    }

    permissions
}

/// Applies one lifecycle event, with the reason recorded.
fn step(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    event: LifecycleEvent,
    actor: Actor,
    reason: &str,
) -> Result<()> {
    let applied = manifest
        .apply(TransitionRequest::new(event, actor, reason))
        .with_context(|| {
            format!(
                "cannot {event} {} while it is {}",
                manifest.id,
                manifest.lifecycle.state().headline().to_lowercase()
            )
        })?;

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

/// The provider to generate with.
fn provider(name: &str) -> Result<Box<dyn AgentProvider>> {
    match name {
        "mock" => Ok(Box::new(MockProvider::new())),
        "anthropic" => Ok(Box::new(
            ephemeral_provider_anthropic::AnthropicProvider::new(),
        )),
        other => bail!(
            "there is no provider called {other}. `mock` produces a fixed example \
             application without a credential, a network connection or a bill; `anthropic` \
             uses a hosted model and needs {}.",
            ephemeral_provider_anthropic::API_KEY_VARIABLE
        ),
    }
}

/// Writes generated source to disk and builds it in a container.
struct DockerBuilder<'a> {
    runtime: &'a DockerRuntime,
    source_dir: PathBuf,
    app: ephemeral_core::AppId,
}

impl Builder for DockerBuilder<'_> {
    fn build(&self, app: &GeneratedApp, files: &[SourceFile]) -> Result<(), String> {
        self.write(app, files)?;

        let request = BuildRequest {
            app: self.app.clone(),
            // Not the version digest: this tag is overwritten on every repair
            // attempt, and a digest that changed under a tag would be a lie.
            // The recorded version is taken once the build finally works.
            // Provisional. The version digest covers the *repaired* source and
            // is not known until the build finally works, so the image is
            // renamed afterwards rather than tagged with a digest that might
            // still change.
            version: BUILDING_TAG.to_owned(),
            context: self.source_dir.clone(),
            dockerfile: self.source_dir.join(DOCKERFILE),
        };

        let image = self.runtime.build_image(&request).map_err(|error| {
            // The full builder output, not the summary — this is what a repair
            // attempt reads.
            match error {
                ephemeral_runtime::RuntimeError::BuildFailed { output, .. } => output,
                other => other.to_string(),
            }
        })?;

        self.test(&image, app)
    }
}

impl DockerBuilder<'_> {
    /// Writes the generated files, refusing anything that escapes the source
    /// directory.
    fn write(&self, app: &GeneratedApp, files: &[SourceFile]) -> Result<(), String> {
        // Validated again here rather than trusted from the caller. This is the
        // last point before a model's path becomes a real one, and the cost of
        // checking twice is nothing next to the cost of not checking at all.
        for file in files {
            if !file.is_safe_path() {
                return Err(format!(
                    "{} is not a path inside the application, and was not written",
                    file.path
                ));
            }
        }

        for file in files {
            let target = self.source_dir.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
            }
            std::fs::write(&target, &file.contents)
                .map_err(|error| format!("could not write {}: {error}", target.display()))?;
        }

        std::fs::write(self.source_dir.join(DOCKERFILE), &app.dockerfile)
            .map_err(|error| format!("could not write the Dockerfile: {error}"))
    }

    /// Runs the application's own tests in a container.
    ///
    /// Without network, without mounts, and read-only — the tests are generated
    /// code too, and run under the same confinement as the application itself.
    fn test(&self, image: &str, app: &GeneratedApp) -> Result<(), String> {
        let mut spec = ephemeral_runtime::ContainerSpec::minimal(
            self.app.clone(),
            image,
            app.test_command.clone(),
        );
        "/app".clone_into(&mut spec.working_dir);

        match self.runtime.run_once(&spec) {
            Ok(output) if output.succeeded => Ok(()),
            Ok(output) => Err(output.output),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Tells the user what was built and what it will want.
fn report(manifest: &AppManifest, outcome: &Outcome, delta: &ephemeral_core::PermissionDelta) {
    println!();
    println!("{} {}", output::good("Built."), manifest.id);
    println!("{}", output::dim(&outcome.describe()));

    if let Some(version) = manifest.current_version() {
        println!("{}", output::field("version", version.digest.short()));
    }

    println!();
    println!("{}", output::dim("What it will ask for"));

    let requested = manifest.permissions.capabilities();
    if requested.is_empty() {
        println!(
            "  {}",
            output::dim("Nothing. It can run with no access at all.")
        );
    } else {
        for permission in &requested {
            println!(
                "  {} {}",
                output::risk(permission.risk()),
                permission.describe()
            );
        }
        println!();
        println!(
            "{}",
            output::dim(&format!(
                "It has none of these yet. `ephemeral permissions {}` shows what it wants; \
                 `ephemeral grant` is how it gets any of it.",
                manifest.id
            ))
        );
    }

    // A widening delta only exists on a regeneration, which is refused above —
    // but the check is here rather than assumed, because that will change.
    if delta.widens() {
        println!();
        println!("{} {}", output::warn("Note:"), delta.describe());
    }
}

#[cfg(test)]
mod tests {
    use ephemeral_agent::{
        Builder, GeneratedApp, MockProvider, RealClock, Run, SourceFile, build::NeverCancelled,
        generate as run_loop, mock::Behaviour,
    };
    use ephemeral_core::{
        AppId, AppManifest, LifecycleState,
        lifecycle::TransitionRequest,
        manifest::GenerationBudget,
        permission::{AppPermission, PathScope},
    };

    use super::*;

    /// A builder that succeeds without touching anything.
    struct AlwaysBuilds;

    impl Builder for AlwaysBuilds {
        fn build(&self, _app: &GeneratedApp, _files: &[SourceFile]) -> Result<(), String> {
            Ok(())
        }
    }

    /// A builder that fails once, then succeeds.
    struct FailsOnce(std::cell::Cell<bool>);

    impl Builder for FailsOnce {
        fn build(&self, _app: &GeneratedApp, _files: &[SourceFile]) -> Result<(), String> {
            if self.0.replace(true) {
                return Ok(());
            }
            Err("SyntaxError: invalid syntax\n".to_owned())
        }
    }

    fn outcome_from(behaviour: Behaviour, builder: &dyn Builder) -> Outcome {
        let budget = GenerationBudget::default();
        let clock = RealClock::start();

        run_loop(
            &MockProvider::with(behaviour),
            "compare two CSV files",
            &Run {
                budget: &budget,
                builder,
                cancellation: &NeverCancelled,
                clock: &clock,
            },
        )
        .expect("the mock and a cooperative builder should produce an application")
    }

    fn requested() -> AppManifest {
        AppManifest::requested(
            AppId::parse("csv-comparator").expect("a valid id"),
            "CSV comparator",
        )
    }

    /// The route has to actually arrive. An event sequence that the state
    /// machine rejects halfway would leave every generated application stranded
    /// in whatever state it got to.
    #[test]
    fn the_success_route_takes_a_requested_application_to_ready() {
        let outcome = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);
        let mut manifest = requested();

        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");

        apply_success(&mut manifest, &outcome, "mock").expect("the route should arrive");

        assert_eq!(manifest.lifecycle.state(), LifecycleState::Ready);
        assert!(manifest.lifecycle.state().is_runnable());
        assert!(
            manifest.runtime.is_some(),
            "a built application knows what it runs on"
        );
    }

    /// Each event must be reported by an actor entitled to report it. The agent
    /// wrote the code; it did not build or test anything, and must not be
    /// recorded as having done so.
    #[test]
    fn every_event_is_raised_by_an_actor_entitled_to_raise_it() {
        let outcome = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);
        let mut manifest = requested();
        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");

        let (recorded, _) =
            apply_success(&mut manifest, &outcome, "mock").expect("the route should arrive");

        for entry in &recorded {
            assert!(
                entry.event.permits(entry.actor),
                "{:?} may not raise {}",
                entry.actor,
                entry.event
            );
        }

        let agent_events: Vec<LifecycleEvent> = recorded
            .iter()
            .filter(|entry| entry.actor == Actor::Agent)
            .map(|entry| entry.event)
            .collect();

        assert_eq!(
            agent_events,
            vec![LifecycleEvent::GenerationCompleted],
            "the agent reports its own work and nothing else"
        );
    }

    /// A repaired application still reaches Ready, and its history says how
    /// many attempts it took rather than hiding them.
    #[test]
    fn a_repaired_application_reaches_ready_and_says_so() {
        let outcome = outcome_from(
            Behaviour::FailsThenRepairs,
            &FailsOnce(std::cell::Cell::new(false)),
        );

        assert_eq!(outcome.repairs(), 1);

        let mut manifest = requested();
        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");

        let (recorded, _) =
            apply_success(&mut manifest, &outcome, "mock").expect("the route should arrive");

        assert_eq!(manifest.lifecycle.state(), LifecycleState::Ready);

        let build = recorded
            .iter()
            .find(|entry| entry.event == LifecycleEvent::BuildSucceeded)
            .expect("a successful route builds");

        assert!(build.reason.contains("1 fix"), "{}", build.reason);
    }

    /// The manifest records what the application *asks for*. Nothing in
    /// generation may put anything in the ledger.
    #[test]
    fn generation_records_requests_and_grants_nothing() {
        let outcome = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);
        let requested = requested_permissions(&outcome.app);

        let capabilities = requested.capabilities();
        assert!(
            !capabilities.is_empty(),
            "the mock asks to read a directory"
        );
        assert!(capabilities.contains(&AppPermission::read(
            PathScope::parse("~/Downloads/**").expect("a valid scope")
        )));
    }

    /// Requesting the same thing twice must not read as wanting it more.
    #[test]
    fn repeated_requests_are_recorded_once() {
        let mut permissions = AppPermissions::none();
        let scope = PathScope::parse("~/Downloads/**").expect("a valid scope");

        permissions.request(&AppPermission::read(scope.clone()));
        permissions.request(&AppPermission::read(scope));

        assert_eq!(permissions.capabilities().len(), 1);
    }

    /// A generated application's identity has to be recorded, or nothing later
    /// can say what it was.
    #[test]
    fn generation_records_a_version() {
        let outcome = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);
        let mut manifest = requested();

        assert_eq!(manifest.current_version(), None);
        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");
        apply_success(&mut manifest, &outcome, "mock").expect("the route should arrive");

        let version = manifest.current_version().expect("a version was recorded");
        assert_eq!(version.sequence, 1);
        assert!(!version.digest.short().is_empty());
        assert!(
            !version.requests.is_empty(),
            "the version records what it asks for, so an update can be compared"
        );
    }

    /// Two runs of the same deterministic provider produce the same
    /// application, which is what makes the digest an identity rather than a
    /// timestamp.
    #[test]
    fn the_same_generation_produces_the_same_digest() {
        let one = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);
        let other = outcome_from(Behaviour::Succeeds, &AlwaysBuilds);

        assert_eq!(one.recipe("cpu=500"), other.recipe("cpu=500"));
    }

    /// The question ADR-0011 exists to answer, reachable from a regeneration:
    /// a version that wants more must not inherit the approval given to the one
    /// before it.
    #[test]
    fn a_widening_update_withdraws_the_grants_it_would_have_inherited() {
        use ephemeral_core::{
            Principal,
            permission::{HostScope, Permission},
            storage::Workspace,
        };

        let home = tempfile::tempdir().expect("a temporary directory");
        let mut workspace = Workspace::open(home.path()).expect("a workspace");
        let app = AppId::parse("csv-comparator").expect("a valid id");

        let reading = AppPermission::read(PathScope::parse("~/Downloads/**").expect("a scope"));
        let calling = AppPermission::outbound(HostScope::parse("*").expect("a host"));

        for permission in [&reading, &calling] {
            workspace
                .ledger_mut()
                .allow(
                    Principal::app(app.clone()),
                    Permission::App(permission.clone()),
                    Actor::User,
                    "agreed earlier",
                )
                .expect("the user may grant");
        }

        // The update newly asks for the network, and nothing else.
        let delta = ephemeral_core::PermissionDelta {
            added: vec![calling.clone()],
            removed: Vec::new(),
        };

        let withdrawn = withdraw_widened(&mut workspace, &app, &delta);

        assert!(
            withdrawn > 0,
            "the newly requested capability loses its grant"
        );
        assert!(
            !workspace.ledger().check_app(&app, &calling).is_allowed(),
            "what the update newly wants must not be inherited"
        );
        assert!(
            workspace
                .ledger()
                .active_grants(&Principal::app(app.clone()))
                .iter()
                .any(|grant| grant.permission == Permission::App(reading.clone())
                    && grant.decision.is_allowed()),
            "an unrelated decision the user already made must survive"
        );
    }

    /// An update that asks for nothing new costs the user nothing.
    #[test]
    fn an_unchanged_update_withdraws_nothing() {
        use ephemeral_core::storage::Workspace;

        let home = tempfile::tempdir().expect("a temporary directory");
        let mut workspace = Workspace::open(home.path()).expect("a workspace");
        let app = AppId::parse("csv-comparator").expect("a valid id");

        let withdrawn = withdraw_widened(
            &mut workspace,
            &app,
            &ephemeral_core::PermissionDelta::default(),
        );

        assert_eq!(withdrawn, 0);
    }

    /// An unknown provider names the ones that exist, rather than leaving
    /// somebody to guess.
    #[test]
    fn an_unknown_provider_says_what_does_exist() {
        assert!(provider("mock").is_ok(), "the mock provider should exist");
        assert!(
            provider("anthropic").is_ok(),
            "a real provider should be constructible even without a credential — \
             the missing key is reported by `availability`, not by construction"
        );

        let error = match provider("gpt") {
            Ok(_) => panic!("there is no provider called gpt"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("no provider called gpt"), "{error}");
        assert!(error.contains("mock"), "{error}");
        assert!(error.contains("anthropic"), "{error}");
    }
}
