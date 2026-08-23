//! Turning a recorded intent into an application: plan, write, build, test,
//! repair.
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

use std::path::PathBuf;

use anyhow::{Context as _, Error, Result, bail};
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

use crate::sandbox::usable_runtime;

/// One capability a generated application will ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requested {
    /// What it wants, completing "… wants to".
    pub wants: String,

    /// How dangerous it would be.
    pub risk: String,
}

/// What a generation run produced, and what a person should be told about it.
///
/// The sentences are here rather than in each client, for the reason every
/// other phrase in the shared layers is: a run that withdrew two grants must
/// say so the same way in a window as in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// Which application.
    pub app: String,

    /// What happened, in a few words.
    pub headline: String,

    /// How it went — attempts, tokens, cost — in the loop's own words.
    pub how_it_went: String,

    /// The version this produced.
    pub version: Option<String>,

    /// What the application will ask to be allowed to do.
    ///
    /// It holds none of it yet: generating writes code, it does not grant
    /// anything.
    pub requests: Vec<Requested>,

    /// How many repair rounds it took.
    pub repairs: u32,

    /// What this version wants that the one before it did not, if anything.
    pub widened: Option<String>,

    /// How many grants were withdrawn because it widened.
    pub grants_withdrawn: usize,

    /// Said only when regenerating changed nothing about what is asked for.
    pub unchanged: Option<String>,

    /// Anything that went wrong without failing the run.
    pub warnings: Vec<String>,
}

/// The file a build recipe is written to.
const DOCKERFILE: &str = "Dockerfile";

/// The provisional tag a build carries before its version is known.
const BUILDING_TAG: &str = "building";

/// The event that starts a run, or `None` if this application cannot start one.
///
/// Two events lead to planning, and which one applies depends on where the
/// application has been. `Plan` is for one that has never been generated;
/// `Retry` is for one that has, and whose last attempt ended somewhere it can
/// be picked up from — a failed build, a cancelled run, a blocker the user has
/// resolved, or a rollback, which clears the built image and leaves an
/// application that has source and nothing to run.
///
/// Checking only for `Plan` meant every one of those answered "cannot
/// regenerate: it is blocked", including the rollback whose own advice is to
/// generate again.
fn starting_event(manifest: &AppManifest) -> Option<LifecycleEvent> {
    [LifecycleEvent::Plan, LifecycleEvent::Retry]
        .into_iter()
        .find(|event| manifest.lifecycle.can_apply(*event, Actor::Ephemeral))
}

/// Builds an application from the intent already recorded for it.
///
/// # Errors
///
/// If this application cannot start a run from the state it is in, if Ephemeral
/// has not been allowed to do what the chosen provider needs, if the provider
/// or the container runtime is unusable, or if the run itself fails — a model
/// that will not answer, a build that will not build after its repairs, a
/// ceiling reached. A failed run is still recorded: what happened is written to
/// the manifest before the error comes back.
pub fn generate(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    provider_name: &str,
) -> Result<Generated> {
    let regenerating = manifest.runtime.is_some();

    let Some(start) = starting_event(manifest) else {
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
    };

    // Ephemeral's own authority, before anything is spent or spawned. What a
    // run needs depends on which provider it is: a model on this machine costs
    // nothing and reaches nowhere, and asking for network access to use one
    // would be asking for something that is not going to happen (ADR-0003).
    for required in provider_authority(provider_name) {
        ephemeral_api::authority::require(workspace.ledger(), &required).map_err(Error::msg)?;
    }

    let provider = provider(provider_name)?;
    provider
        .availability()
        .with_context(|| format!("{provider_name} cannot be used"))?;

    let runtime = usable_runtime(workspace)?;

    let source_dir = workspace.layout().app(&manifest.id).source();
    std::fs::create_dir_all(&source_dir)
        .with_context(|| format!("could not create {}", source_dir.display()))?;

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
        workspace,
        manifest,
        start,
        Actor::Ephemeral,
        &format!("planning with {}", provider.name()),
    )?;
    workspace.apps_mut().save(manifest)?;
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
            workspace,
            manifest,
            &outcome,
            provider.name(),
            regenerating,
            &runtime,
        ),
        Err(error) => {
            fail(workspace, manifest, &error)?;
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
        // What the application says it takes, so a client can draw a form
        // rather than asking somebody to type a command line.
        inputs: outcome.app.inputs.clone(),
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
) -> Result<Generated> {
    let (recorded, delta) = apply_success(manifest, outcome, provider)?;

    // Keep this version's source before anything overwrites it. A digest in the
    // history that nothing can restore is half a promise: it says what the
    // application was and cannot put it back. Failing to keep it is not worth
    // failing a successful generation over, so it is reported and the run
    // stands — but it is reported, rather than leaving somebody to discover it
    // when a rollback has nothing to roll back to.
    let mut warnings = Vec::new();
    if let Some(version) = manifest.current_version()
        && let Err(error) = workspace.apps().keep_version(&manifest.id, &version.digest)
    {
        warnings.push(format!(
            "This version's source was not kept, so you will not be able to return to it: {error}"
        ));
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
        ephemeral_api::withdraw_widened(workspace, &manifest.id, &delta)
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

    Ok(Generated {
        app: manifest.id.to_string(),
        headline: format!("Built. {}", manifest.id),
        how_it_went: outcome.describe(),
        version: manifest
            .current_version()
            .map(|version| version.digest.short().to_owned()),
        requests: manifest
            .permissions
            .capabilities()
            .iter()
            .map(|permission| Requested {
                wants: permission.describe(),
                risk: permission.risk().as_str().to_owned(),
            })
            .collect(),
        repairs: u32::try_from(outcome.repairs()).unwrap_or(u32::MAX),
        widened: delta.widens().then(|| delta.describe()),
        grants_withdrawn: withdrawn,
        unchanged: (!delta.widens() && regenerating).then(|| {
            "This update asks for nothing new, so your existing decisions stand.".to_owned()
        }),
        warnings,
    })
}

/// Records a failed run, so the manifest says what happened.
fn fail(workspace: &mut Workspace, manifest: &mut AppManifest, error: &AgentError) -> Result<()> {
    // A run that got as far as producing code and failed to build is a
    // different story from one that never planned, and the machine can tell
    // them apart only if the events it is given are the ones that happened.
    let route: &[(LifecycleEvent, Actor)] = match error {
        // A provider that could not be reached, would not answer, or answered
        // with something unusable is a blocker: the person fixes the credential
        // or the network and asks again. It is not a build failure — no build
        // was attempted, and saying one failed would be a record of something
        // that did not happen.
        AgentError::Refused(_)
        | AgentError::Unreadable { .. }
        | AgentError::Unavailable { .. }
        | AgentError::Failed { .. } => &[(LifecycleEvent::Block, Actor::Ephemeral)],
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

    // Whatever route was taken, the application must not be left somewhere it
    // can only be deleted from. A run that fails before any code exists cannot
    // take the build-failure route — a manifest with no runtime may not enter
    // `Building`, so the events are refused one at a time and the application
    // is stranded in `Generating`, which offers no way to start again.
    //
    // Found by running it: a rejected API key left an application that could
    // not be generated even after the key was fixed. Blocking is what that
    // situation is — something the person resolves and retries — and `Blocked`
    // offers exactly that.
    if starting_event(manifest).is_none()
        && manifest
            .lifecycle
            .can_apply(LifecycleEvent::Block, Actor::Ephemeral)
    {
        let _ = step(
            workspace,
            manifest,
            LifecycleEvent::Block,
            Actor::Ephemeral,
            &error.to_string(),
        );
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

/// Every provider a client may offer, in the order a person meets them.
///
/// `mock` first because it is the one that works with nothing installed and no
/// account anywhere, and `local` before the hosted two because it is the only
/// one that does not send what somebody asked for to a company.
pub const PROVIDERS: [&str; 4] = ["mock", "local", "anthropic", "openai"];

/// What Ephemeral must be allowed to do before generating with `provider`.
///
/// Nothing for the mock, which is a fixture. Nothing beyond the ordinary for
/// `local`, which talks to a model on this machine over the loopback interface:
/// requiring network access there would ask somebody to allow the one thing
/// that provider exists to avoid. A hosted provider needs both halves of what
/// it actually does — reach the network, and use a credential.
pub fn provider_authority(name: &str) -> Vec<ephemeral_core::permission::MetaPermission> {
    match name {
        "anthropic" | "openai" => vec![
            ephemeral_api::authority::HOSTED_PROVIDER,
            ephemeral_api::authority::CREDENTIAL,
        ],
        _ => Vec::new(),
    }
}

/// The provider to generate with.
///
/// # Errors
///
/// If no provider goes by that name, with the ones that do named in the
/// refusal.
pub fn provider(name: &str) -> Result<Box<dyn AgentProvider>> {
    match name {
        "mock" => Ok(Box::new(MockProvider::new())),
        "anthropic" => Ok(Box::new(
            ephemeral_provider_anthropic::AnthropicProvider::new(),
        )),
        "openai" => Ok(Box::new(ephemeral_provider_openai::OpenAiProvider::new())),
        "local" => Ok(Box::new(ephemeral_provider_local::LocalProvider::new())),
        other => bail!(
            "there is no provider called {other}. `mock` produces a fixed example \
             application without a credential, a network connection or a bill; `local` \
             uses a model on this machine, so the intent does not leave it; `anthropic` \
             needs {}, and `openai` needs {} — or {} pointed at anything that speaks the \
             same format.",
            ephemeral_provider_anthropic::API_KEY_VARIABLE,
            ephemeral_provider_openai::API_KEY_VARIABLE,
            ephemeral_provider_openai::BASE_URL_VARIABLE
        ),
    }
}

/// What a provider says it can be asked for, and whether it can be reached.
///
/// The two questions are one call because they have one answer. Somebody about
/// to spend money on generation wants to know that the credential works and
/// that the model name they are about to use exists; asking those separately
/// gives two ways to be almost-configured, and the second failure arrives after
/// the first token has been paid for.
///
/// Unlike generating, this reaches the network — so it needs the same authority
/// generating does, and asks for it in the same words.
///
/// # Errors
///
/// If Ephemeral has not been allowed to reach a hosted provider, if there is no
/// provider by that name, or if the service refuses. A refusal carries the
/// service's own words, which is the point: "invalid api key" from the vendor
/// beats anything this crate could invent.
pub fn models(workspace: &Workspace, provider_name: &str) -> Result<Vec<ephemeral_agent::Model>> {
    for required in provider_authority(provider_name) {
        ephemeral_api::authority::require(workspace.ledger(), &required).map_err(Error::msg)?;
    }

    let mut listed = provider(provider_name)?.models()?;

    // Sorted, because the order a service returns is the order it happens to
    // store them in. OpenAI answers with 126 models beginning
    // `gpt-4-0613, gpt-4, gpt-3.5-turbo, gpt-live-transcribe` — a list nobody
    // can find anything in. Sorting claims nothing about the models; it just
    // makes the list navigable.
    listed.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(listed)
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
    /// A run that fails must leave the application somewhere it can be picked
    /// up from. This was found with a real credential: Anthropic rejected the
    /// key, the failure took the build-failure route, `GenerationCompleted` was
    /// refused because a manifest with no runtime may not enter `Building`, and
    /// the application sat in `Generating` — which offers no way to start again
    /// — so fixing the key changed nothing and the only way out was to delete
    /// it and describe what you wanted a second time.
    #[test]
    fn a_run_that_fails_before_any_code_exists_can_be_tried_again() {
        use ephemeral_core::storage::Workspace;

        let home = tempfile::tempdir().expect("a temporary directory");
        let mut workspace = Workspace::open(home.path()).expect("a workspace");
        let mut manifest = requested();
        workspace.apps_mut().create(&manifest).expect("created");

        // As far as a run gets before it asks a model anything.
        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");

        fail(
            &mut workspace,
            &mut manifest,
            &AgentError::Failed {
                provider: "anthropic".to_owned(),
                reason: "API key is invalid.".to_owned(),
            },
        )
        .expect("a failed run is still recorded");

        assert_eq!(
            manifest.lifecycle.state(),
            LifecycleState::Blocked,
            "a provider that refused is something to resolve, not a build that failed"
        );
        assert_eq!(
            starting_event(&manifest),
            Some(LifecycleEvent::Retry),
            "and fixing what blocked it has to be enough to try again"
        );
    }

    /// A rollback's own advice is "generate again to rebuild", and for as long
    /// as this looked only for `Plan`, that advice was impossible to follow:
    /// every state a rolled-back or failed application is in offers `Retry`
    /// instead, and the answer was "cannot regenerate: it is blocked".
    #[test]
    fn a_run_starts_from_whichever_event_this_application_can_actually_raise() {
        let fresh = requested();
        assert_eq!(
            starting_event(&fresh),
            Some(LifecycleEvent::Plan),
            "an application nobody has generated plans"
        );

        let mut failed = requested();
        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildFailed, Actor::Runtime),
        ] {
            // Building means knowing what it runs on, which generation records
            // before it hands anything to the builder.
            if event == LifecycleEvent::GenerationCompleted {
                failed.runtime = Some(ephemeral_core::manifest::RuntimeSpec::docker_job(
                    "python:3.12-slim",
                    vec!["python".to_owned()],
                ));
            }
            failed
                .apply(TransitionRequest::new(event, actor, "the build broke"))
                .expect("the route to a failed build");
        }
        assert_eq!(
            starting_event(&failed),
            Some(LifecycleEvent::Retry),
            "an application that has been here before retries"
        );

        let mut ready = requested();
        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Ephemeral),
        ] {
            // A manifest reaches Ready only with something to run, so the
            // runtime is set on the way, as generation sets it.
            if event == LifecycleEvent::GenerationCompleted {
                ready.runtime = Some(ephemeral_core::manifest::RuntimeSpec::docker_job(
                    "python:3.12-slim",
                    vec!["python".to_owned()],
                ));
            }
            ready
                .apply(TransitionRequest::new(event, actor, "generating"))
                .expect("the route to ready");
        }
        assert_eq!(
            starting_event(&ready),
            None,
            "a ready application is running code somebody approved; it is stopped or archived \
             before it is replaced"
        );
    }

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

        let withdrawn = ephemeral_api::withdraw_widened(&mut workspace, &app, &delta);

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

        let withdrawn = ephemeral_api::withdraw_widened(
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

        for real in ["anthropic", "openai", "local"] {
            assert!(
                provider(real).is_ok(),
                "a real provider should be constructible even without a credential — \
                 the missing key is reported by `availability`, not by construction: {real}"
            );
        }

        let error = match provider("gpt") {
            Ok(_) => panic!("there is no provider called gpt"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("no provider called gpt"), "{error}");
        for named in ["mock", "local", "anthropic", "openai"] {
            assert!(error.contains(named), "{error}");
        }
    }

    /// Every provider the CLI can build names itself, because that name is
    /// written into the audit record and into the manifest's version history.
    #[test]
    fn every_provider_names_itself() {
        for name in ["mock", "anthropic", "openai", "local"] {
            let built = provider(name).expect("the provider exists");

            assert_eq!(built.name(), name);
        }
    }
}
