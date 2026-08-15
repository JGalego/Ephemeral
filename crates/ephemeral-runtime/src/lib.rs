//! # Ephemeral runtime
//!
//! The part of Ephemeral that actually confines a generated application.
//!
//! Everything in [`ephemeral-core`] decides *what an application is allowed to
//! do*. This crate is where that decision becomes a fact about a running
//! process. It is therefore the crate whose bugs are vulnerabilities rather than
//! defects, and it is written accordingly:
//!
//! - **The sandbox is data before it is an action.** A [`spec::ContainerSpec`]
//!   is built from granted permissions, and turned into an argument vector by a
//!   pure function ([`docker::command`]). Every hardening flag is therefore a
//!   unit-testable assertion that runs in CI, where no container daemon exists
//!   ([ADR-0014]).
//! - **Access is built from grants, never from the manifest.** The manifest says
//!   what an application *wants*. Building the sandbox from it would let an
//!   application widen its own confinement by asking.
//! - **Omission yields less access.** Every default here is the closed one: no
//!   network, no mounts, no published ports, no environment, non-root, all
//!   capabilities dropped. A control this crate forgets to add is a control the
//!   application does not get.
//! - **A control that cannot be enforced is a refusal.** If Ephemeral cannot
//!   apply a limit it promised, it does not start the application unlimited; it
//!   returns [`RuntimeError::CannotEnforce`] and says which control and why.
//!
//! ## What lives here
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`spec`] | What is to be run, and under what confinement |
//! | [`docker`] | The Docker implementation, driven through the `docker` command |
//!
//! [`ephemeral-core`]: ephemeral_core
//! [ADR-0014]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0014-drive-docker-through-its-cli.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod docker;
pub mod spec;

use std::fmt;

use ephemeral_core::AppId;

pub use spec::{AccessPlan, ContainerSpec, Egress, HostPaths, Mount, PortBinding, RefusedAccess};

/// The prefix Ephemeral puts on every container and volume it creates.
///
/// Cleanup finds Ephemeral's containers by this prefix and by the labels in
/// [`MANAGED_LABEL`]. Reaping by a bare application id would be an efficient way
/// to destroy somebody's unrelated work, so nothing in this crate ever addresses
/// a container by an unprefixed name.
pub const CONTAINER_PREFIX: &str = "ephemeral-";

/// The label marking a container as Ephemeral's to manage.
pub const MANAGED_LABEL: &str = "sh.ephemeral.managed";

/// The label recording which application a container belongs to.
pub const APP_LABEL: &str = "sh.ephemeral.app";

/// Something went wrong while running, or refusing to run, an application.
///
/// These messages are shown to people, so each one names what failed and what
/// to do about it. "Docker error" tells a user nothing; "Docker is installed but
/// not running — start Docker Desktop and try again" tells them everything.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// The runtime is not installed on this machine.
    #[error("{runtime} is not installed. {remedy}")]
    NotInstalled {
        /// Which runtime was looked for.
        runtime: &'static str,
        /// What the user can do about it.
        remedy: String,
    },

    /// The runtime is installed but not currently usable.
    #[error("{runtime} is installed but not responding. {remedy}")]
    NotResponding {
        /// Which runtime.
        runtime: &'static str,
        /// What the user can do about it.
        remedy: String,
    },

    /// A confinement Ephemeral promised cannot be applied here.
    ///
    /// The application is **not** started. This is the variant that keeps the
    /// product honest: it is always safe to add a control this crate cannot
    /// enforce, because the result is a refusal rather than an unconfined
    /// application.
    #[error("cannot enforce {control}: {reason}")]
    CannotEnforce {
        /// The control that could not be applied, in the user's language.
        control: String,
        /// Why not, and what it would take.
        reason: String,
    },

    /// The application is not in a state where this makes sense.
    #[error("{app} is not running")]
    NotRunning {
        /// Which application.
        app: AppId,
    },

    /// Something is already running under this application's name.
    #[error("{app} is already running")]
    AlreadyRunning {
        /// Which application.
        app: AppId,
    },

    /// The image could not be obtained.
    #[error("could not get the image {image}: {reason}")]
    ImageUnavailable {
        /// The image reference.
        image: String,
        /// What the runtime said.
        reason: String,
    },

    /// A runtime command failed.
    #[error("`{command}` failed ({status}): {stderr}")]
    CommandFailed {
        /// The command line, as run. Never contains a secret value — see
        /// [`spec::ContainerSpec::environment_names`].
        command: String,
        /// How it exited.
        status: String,
        /// What it printed.
        stderr: String,
    },

    /// A runtime command could not be started at all.
    #[error("could not run `{command}`: {source}")]
    CommandUnavailable {
        /// The command that could not be spawned.
        command: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// An image could not be built from the application's source.
    ///
    /// Carries the builder's output verbatim, because that output is the input
    /// to a repair attempt. Summarising it here would throw away the only thing
    /// that can fix the problem.
    #[error("could not build {app}: {summary}")]
    BuildFailed {
        /// Which application.
        app: AppId,
        /// The first line of what went wrong, for a person.
        summary: String,
        /// Everything the builder printed, for a repair attempt.
        output: String,
    },

    /// The runtime's output was not what this version of Ephemeral understands.
    #[error("could not understand the output of `{command}`: {reason}")]
    UnreadableOutput {
        /// The command whose output was parsed.
        command: String,
        /// What was wrong with it.
        reason: String,
    },
}

/// Whether a runtime can be used, and if not, what to tell the user.
///
/// `ephemeral doctor` reports this. Detection is separate from use so that a
/// missing runtime is a diagnosis with a remedy rather than a failure at the
/// moment somebody tries to run something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Availability {
    /// Whether applications can be run right now.
    pub usable: bool,

    /// The runtime's own version string, when it could be asked.
    pub version: Option<String>,

    /// What a person should be told — either confirmation or a remedy.
    pub explanation: String,
}

impl Availability {
    /// A runtime that is ready.
    #[must_use]
    pub fn usable(version: impl Into<String>) -> Self {
        let version = version.into();
        Self {
            usable: true,
            explanation: format!("{version} is available. Applications will run in containers."),
            version: Some(version),
        }
    }

    /// A runtime that is not ready, with the reason and the remedy.
    #[must_use]
    pub fn unusable(explanation: impl Into<String>) -> Self {
        Self {
            usable: false,
            version: None,
            explanation: explanation.into(),
        }
    }
}

/// What a container is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContainerState {
    /// Created but not started.
    Created,
    /// Executing.
    Running,
    /// Suspended, holding its memory.
    Paused,
    /// Shutting down.
    Stopping,
    /// Finished or stopped.
    Exited,
    /// Present but in a state the runtime itself calls broken.
    Dead,
    /// No container exists under this application's name.
    Absent,
}

impl ContainerState {
    /// Whether the application is consuming CPU right now.
    #[must_use]
    pub fn is_live(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether a container exists at all, in any state.
    ///
    /// The distinction matters for teardown: an exited container still occupies
    /// a name and disk, so "not running" is not "nothing to clean up".
    #[must_use]
    pub fn exists(self) -> bool {
        !matches!(self, Self::Absent)
    }

    /// Parses Docker's `State.Status` field.
    ///
    /// An unrecognised status maps to [`ContainerState::Dead`] rather than to
    /// something benign: a state this version does not understand is not a state
    /// it should report as healthy.
    #[must_use]
    pub fn from_docker_status(status: &str) -> Self {
        match status {
            "created" => Self::Created,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "removing" => Self::Stopping,
            "exited" => Self::Exited,
            _ => Self::Dead,
        }
    }
}

impl fmt::Display for ContainerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopping => "stopping",
            Self::Exited => "exited",
            Self::Dead => "dead",
            Self::Absent => "absent",
        })
    }
}

/// What the runtime knows about one application's container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerStatus {
    /// Which application.
    pub app: AppId,

    /// What it is doing.
    pub state: ContainerState,

    /// The runtime's own identifier, when a container exists.
    pub container_id: Option<String>,

    /// The exit code, once it has one.
    pub exit_code: Option<i64>,

    /// The health check's verdict, when the image defines one.
    pub health: Option<String>,
}

impl ContainerStatus {
    /// The status of an application with no container.
    #[must_use]
    pub fn absent(app: AppId) -> Self {
        Self {
            app,
            state: ContainerState::Absent,
            container_id: None,
            exit_code: None,
            health: None,
        }
    }

    /// Whether this container finished successfully.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.state == ContainerState::Exited && self.exit_code == Some(0)
    }

    /// Whether this container should move the application to `Unhealthy`.
    ///
    /// A failing health check and a non-zero exit are both attention-worthy; a
    /// clean exit is not.
    #[must_use]
    pub fn is_unhealthy(&self) -> bool {
        self.health.as_deref() == Some("unhealthy")
            || matches!(self.state, ContainerState::Dead)
            || (self.state == ContainerState::Exited
                && self.exit_code.is_some_and(|code| code != 0))
    }
}

/// A container Ephemeral created and may need to clean up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedContainer {
    /// The container's name.
    pub name: String,

    /// The application it was created for, if the label is still readable.
    pub app: Option<AppId>,

    /// What it is doing.
    pub state: ContainerState,
}

/// Somewhere a generated application can be run under confinement.
///
/// The trait exists so that the Docker implementation, the weaker native
/// implementation, and the remote one used by mobile ([ADR-0007]) present the
/// same surface — and so that everything above this line can be tested without a
/// container daemon.
///
/// Implementations must uphold two properties. **Refuse rather than degrade:** a
/// confinement that cannot be applied produces
/// [`RuntimeError::CannotEnforce`], never a started application with a missing
/// control. **Never widen a spec:** an implementation may reject anything in a
/// [`ContainerSpec`], but may not grant access the spec does not contain.
///
/// [ADR-0007]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0007-mobile-control-plane.md
pub trait Runtime {
    /// The name of this runtime, for the interface and the audit log.
    fn name(&self) -> &'static str;

    /// Whether this runtime can be used, and what to tell the user if not.
    ///
    /// Must not fail: an unavailable runtime is a diagnosis, not an error.
    fn availability(&self) -> Availability;

    /// Makes an image available locally, so that starting is not a download.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::ImageUnavailable`] if the image cannot be obtained.
    fn prepare_image(&self, image: &str) -> Result<(), RuntimeError>;

    /// Builds an application's image from source it wrote.
    ///
    /// Returns the image reference to run. Building is where a model's output
    /// first executes anything, so it is the runtime's job rather than the
    /// generator's: the build runs under the same daemon, with the same labels,
    /// and produces something Ephemeral can find and destroy later.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::BuildFailed`] with the output, which is the input to a
    /// repair attempt and therefore has to be preserved rather than summarised.
    fn build_image(&self, request: &BuildRequest) -> Result<String, RuntimeError>;

    /// Runs a container to completion and returns what it printed.
    ///
    /// For work that finishes on its own — running an application's tests,
    /// above all. Confined exactly as [`Runtime::start`] would confine it: a
    /// generated test is generated code, and gets no more of the machine than
    /// the application does.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CannotEnforce`] if the confinement cannot be applied, or
    /// [`RuntimeError::CommandFailed`] if the runtime refuses. A container that
    /// runs and exits non-zero is **not** an error — that is an [`Completed`]
    /// with `succeeded` false, because a failing test is an answer rather than
    /// a malfunction.
    fn run_once(&self, spec: &ContainerSpec) -> Result<Completed, RuntimeError>;

    /// Gives a built image its final name, once the version is known.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if the runtime refuses.
    fn name_image(&self, built: &str, app: &AppId, version: &str) -> Result<String, RuntimeError>;

    /// Starts an application under the confinement described by `spec`.
    ///
    /// `secrets` supplies the values for [`ContainerSpec::environment_names`].
    /// They are passed to the child process's environment and never appear in
    /// an argument vector, so nothing in this path can leak a secret value into
    /// the process table, an error message or the audit log.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CannotEnforce`] if any confinement in `spec` cannot be
    /// applied here, [`RuntimeError::AlreadyRunning`] if the application is
    /// already up, and [`RuntimeError::CommandFailed`] if the runtime rejects
    /// the request.
    fn start(
        &self,
        spec: &ContainerSpec,
        secrets: &Secrets,
    ) -> Result<ContainerStatus, RuntimeError>;

    /// Asks an application to stop, then stops it.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if the runtime refuses.
    fn stop(&self, app: &AppId) -> Result<(), RuntimeError>;

    /// Suspends a running application without losing its state.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::NotRunning`] if there is nothing to pause.
    fn pause(&self, app: &AppId) -> Result<(), RuntimeError>;

    /// Resumes a suspended application.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::NotRunning`] if there is nothing to resume.
    fn resume(&self, app: &AppId) -> Result<(), RuntimeError>;

    /// What this application's container is doing.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if the runtime cannot be asked.
    fn status(&self, app: &AppId) -> Result<ContainerStatus, RuntimeError>;

    /// The most recent `lines` lines the application produced.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if the logs cannot be read.
    fn logs(&self, app: &AppId, lines: u32) -> Result<String, RuntimeError>;

    /// Removes the container and everything it holds.
    ///
    /// Idempotent: removing what is not there succeeds.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if removal is refused.
    fn remove(&self, app: &AppId) -> Result<(), RuntimeError>;

    /// Every container Ephemeral created that still exists.
    ///
    /// The input to orphan cleanup: containers left behind by a crash, or by an
    /// application deleted while it was running.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CommandFailed`] if the runtime cannot be asked.
    fn managed_containers(&self) -> Result<Vec<ManagedContainer>, RuntimeError>;
}

/// What a container that ran to completion produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    /// Whether it exited zero.
    pub succeeded: bool,

    /// How it exited.
    pub exit_code: i32,

    /// Everything it printed, both streams interleaved.
    ///
    /// Kept whole. This is what a person reads when a test fails and what a
    /// repair attempt is given, and either use is defeated by a summary.
    pub output: String,
}

/// What to build, and from where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    /// Which application.
    pub app: AppId,

    /// The version being built, which becomes part of the image tag.
    ///
    /// Two builds of different source must never share a tag, or a recorded
    /// version stops meaning anything.
    pub version: String,

    /// The directory handed to the builder as its context.
    pub context: std::path::PathBuf,

    /// The build recipe, named explicitly.
    ///
    /// Found by Ephemeral rather than by convention, so a stray `Dockerfile`
    /// elsewhere in a model-generated tree cannot become the one that is built.
    pub dockerfile: std::path::PathBuf,
}

/// Values for the settings an application was granted access to.
///
/// A deliberately opaque holder rather than a plain map. It has no `Debug`,
/// `Clone`, `Serialize` or accessor that yields every value at once, so the
/// ordinary ways a secret ends up in a log — formatting a struct, serialising a
/// request, iterating for a summary — are not available.
#[derive(Default)]
pub struct Secrets {
    values: std::collections::BTreeMap<String, String>,
}

impl Secrets {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one value.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }

    /// The value for one name, if there is one.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// How many values are held. The only quantity safe to log.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether anything is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Prints the count and nothing else.
///
/// Written by hand because the derived implementation would print every value,
/// and this type exists to hold values that must never be printed.
impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secrets")
            .field("count", &self.values.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The type that holds secret values must not be able to print one, however
    /// it is formatted.
    #[test]
    fn secrets_never_print_their_values() {
        let mut secrets = Secrets::new();
        secrets.insert("API_KEY", "sk-live-do-not-log-this");

        let printed = format!("{secrets:?}");
        assert!(!printed.contains("sk-live"), "{printed}");
        assert!(
            printed.contains('1'),
            "the count is the only thing worth showing"
        );
    }

    /// A state this version does not recognise must not be reported as healthy.
    #[test]
    fn an_unknown_container_state_is_not_treated_as_fine() {
        assert_eq!(
            ContainerState::from_docker_status("running"),
            ContainerState::Running
        );
        assert_eq!(
            ContainerState::from_docker_status("exited"),
            ContainerState::Exited
        );

        let unknown = ContainerState::from_docker_status("some-future-status");
        assert!(!unknown.is_live());
        assert_eq!(unknown, ContainerState::Dead);
    }

    /// "Not running" is not "nothing to clean up" — an exited container still
    /// holds a name and disk.
    #[test]
    fn an_exited_container_still_exists() {
        assert!(ContainerState::Exited.exists());
        assert!(!ContainerState::Exited.is_live());
        assert!(!ContainerState::Absent.exists());
    }

    #[test]
    fn a_failed_exit_is_unhealthy_and_a_clean_one_is_not() {
        let app = AppId::parse("csv-comparator").unwrap();

        let clean = ContainerStatus {
            state: ContainerState::Exited,
            exit_code: Some(0),
            ..ContainerStatus::absent(app.clone())
        };
        assert!(clean.succeeded());
        assert!(!clean.is_unhealthy());

        let failed = ContainerStatus {
            exit_code: Some(1),
            ..clean.clone()
        };
        assert!(!failed.succeeded());
        assert!(failed.is_unhealthy());

        let sick = ContainerStatus {
            state: ContainerState::Running,
            exit_code: None,
            health: Some("unhealthy".to_owned()),
            ..ContainerStatus::absent(app)
        };
        assert!(sick.is_unhealthy());
    }

    /// Every message a user sees has to say what to do next, not merely what
    /// failed.
    #[test]
    fn unenforceable_controls_name_the_control_and_the_reason() {
        let error = RuntimeError::CannotEnforce {
            control: "the list of sites this app may reach".to_owned(),
            reason: "Docker cannot filter outbound traffic by hostname".to_owned(),
        };
        let message = error.to_string();
        assert!(message.contains("the list of sites"), "{message}");
        assert!(message.contains("cannot filter"), "{message}");
    }
}
