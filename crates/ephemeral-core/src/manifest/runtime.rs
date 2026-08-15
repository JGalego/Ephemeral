//! What an application runs on, and how a person reaches it.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The kind of runtime an application executes in.
///
/// Recorded in the manifest rather than decided at launch, so an application's
/// isolation is a durable, inspectable fact rather than a property of whichever
/// machine happens to be starting it ([ADR-0005]).
///
/// [ADR-0005]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0005-docker-first-runtime-abstraction.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeKind {
    /// A container. The desktop default, and the strongest isolation Ephemeral
    /// offers without a virtual machine.
    Docker,

    /// A constrained local process.
    ///
    /// For the cases that genuinely cannot be containerised. Isolation is
    /// materially weaker than [`RuntimeKind::Docker`], so it demands stricter
    /// permission gating and is labelled as such wherever an app is shown —
    /// SECURITY.md does not pretend the two are equivalent.
    Native,

    /// A sandbox on a control plane rather than on this device.
    ///
    /// Used by mobile, where no local runtime exists ([ADR-0007]). Because the
    /// user's data leaves the device, the interface always shows this.
    ///
    /// [ADR-0007]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0007-mobile-control-plane.md
    Remote,
}

impl RuntimeKind {
    /// Every runtime kind.
    pub const ALL: [Self; 3] = [Self::Docker, Self::Native, Self::Remote];

    /// The machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Native => "native",
            Self::Remote => "remote",
        }
    }

    /// Whether this runtime confines the application in a container.
    #[must_use]
    pub fn is_containerised(self) -> bool {
        matches!(self, Self::Docker | Self::Remote)
    }

    /// Whether the application executes on this device.
    ///
    /// The answer the interface must show honestly: if it is `false`, the user's
    /// data is leaving their machine.
    #[must_use]
    pub fn runs_locally(self) -> bool {
        matches!(self, Self::Docker | Self::Native)
    }

    /// How the isolation is described to a person.
    #[must_use]
    pub fn describe_isolation(self) -> &'static str {
        match self {
            Self::Docker => {
                "This app runs in a container on this device. It can only reach what you \
                 have allowed it to reach."
            }
            Self::Native => {
                "This app runs directly on this device, without a container. That is less \
                 isolated than usual, so it is limited to a smaller set of permissions."
            }
            Self::Remote => {
                "This app runs on Ephemeral's servers, not on this device. Data you give \
                 it leaves your device."
            }
        }
    }
}

impl fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a person uses a generated application.
///
/// Ephemeral does not impose one UI technology on generated apps, but it does
/// need to know how to present each one, so that "open it" means something
/// consistent whatever the app turns out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppInterface {
    /// A web application, opened in a browser or an embedded view.
    Web,

    /// A command-line tool, run with arguments.
    CommandLine,

    /// An HTTP API with no interface of its own.
    Api,

    /// Something that runs in the background on a schedule or a trigger.
    Worker,

    /// A one-off job: it runs, produces something, and stops.
    Job,
}

impl AppInterface {
    /// Every interface kind.
    pub const ALL: [Self; 5] = [
        Self::Web,
        Self::CommandLine,
        Self::Api,
        Self::Worker,
        Self::Job,
    ];

    /// The machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::CommandLine => "command_line",
            Self::Api => "api",
            Self::Worker => "worker",
            Self::Job => "job",
        }
    }

    /// Whether this app is reached by opening something.
    #[must_use]
    pub fn is_openable(self) -> bool {
        matches!(self, Self::Web | Self::Api)
    }

    /// What the primary action on this app should be called.
    #[must_use]
    pub fn primary_action(self) -> &'static str {
        match self {
            Self::Web => "Open",
            Self::CommandLine => "Run",
            Self::Api => "View endpoint",
            Self::Worker => "Start",
            Self::Job => "Run once",
        }
    }
}

impl fmt::Display for AppInterface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything needed to run an application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSpec {
    /// Which runtime.
    #[serde(rename = "type")]
    pub kind: RuntimeKind,

    /// The base image, for containerised runtimes.
    ///
    /// Pinned by digest wherever possible: an image reference that can change
    /// underneath a "reproducible" application is not reproducible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,

    /// The language or platform version the application targets, such as
    /// `python-3.12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// The command that starts the application, already split into arguments.
    ///
    /// A vector rather than a string so that nothing has to be shell-parsed:
    /// there is no shell in the path, so there is no shell injection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entrypoint: Vec<String>,

    /// How a person uses this application.
    pub interface: AppInterface,

    /// The port the application listens on inside its sandbox, if any.
    ///
    /// Where — and whether — that port is published is the runtime's decision,
    /// not the application's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl RuntimeSpec {
    /// A Docker-backed web application.
    #[must_use]
    pub fn docker_web(image: impl Into<String>, port: u16) -> Self {
        Self {
            kind: RuntimeKind::Docker,
            image: Some(image.into()),
            version: None,
            entrypoint: Vec::new(),
            interface: AppInterface::Web,
            port: Some(port),
        }
    }

    /// A Docker-backed one-off job.
    #[must_use]
    pub fn docker_job(image: impl Into<String>, entrypoint: Vec<String>) -> Self {
        Self {
            kind: RuntimeKind::Docker,
            image: Some(image.into()),
            version: None,
            entrypoint,
            interface: AppInterface::Job,
            port: None,
        }
    }

    /// Whether this application runs on the user's own device.
    #[must_use]
    pub fn runs_locally(&self) -> bool {
        self.kind.runs_locally()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_runtime_kind_explains_its_isolation() {
        for kind in RuntimeKind::ALL {
            assert!(
                kind.describe_isolation().len() > 40,
                "{kind} needs a real explanation of what it does and does not confine"
            );
        }
    }

    /// A user must be able to tell, from the manifest alone, whether their data
    /// leaves the device. Hiding that would be a privacy failure, not a UI
    /// simplification.
    #[test]
    fn remote_execution_is_distinguishable_from_local() {
        assert!(RuntimeKind::Docker.runs_locally());
        assert!(RuntimeKind::Native.runs_locally());
        assert!(!RuntimeKind::Remote.runs_locally());
        assert!(
            RuntimeKind::Remote
                .describe_isolation()
                .contains("leaves your device")
        );
    }

    /// The native runtime is the weakest link by construction, and says so.
    #[test]
    fn the_native_runtime_admits_it_is_less_isolated() {
        assert!(!RuntimeKind::Native.is_containerised());
        assert!(
            RuntimeKind::Native
                .describe_isolation()
                .contains("less isolated")
        );
    }

    #[test]
    fn every_interface_has_a_primary_action() {
        for interface in AppInterface::ALL {
            assert!(!interface.primary_action().is_empty());
            assert!(!interface.as_str().is_empty());
        }
        assert_eq!(AppInterface::Web.primary_action(), "Open");
        assert!(AppInterface::Web.is_openable());
        assert!(!AppInterface::Job.is_openable());
    }

    #[test]
    fn runtime_specs_round_trip_through_yaml() {
        for spec in [
            RuntimeSpec::docker_web("python:3.12-slim", 8080),
            RuntimeSpec::docker_job(
                "python:3.12-slim",
                vec!["python".to_owned(), "compare.py".to_owned()],
            ),
        ] {
            let yaml = serde_norway::to_string(&spec).unwrap();
            assert_eq!(serde_norway::from_str::<RuntimeSpec>(&yaml).unwrap(), spec);
            assert!(yaml.contains("type: docker"));
        }
    }

    /// The entrypoint is a list, not a string, so nothing is ever handed to a
    /// shell to re-parse.
    #[test]
    fn the_entrypoint_is_pre_split() {
        let spec = RuntimeSpec::docker_job(
            "alpine",
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "echo $HOME; rm -rf /".to_owned(),
            ],
        );
        let yaml = serde_norway::to_string(&spec).unwrap();
        let parsed: RuntimeSpec = serde_norway::from_str(&yaml).unwrap();

        assert_eq!(parsed.entrypoint.len(), 3);
        assert_eq!(parsed.entrypoint[2], "echo $HOME; rm -rf /");
    }

    #[test]
    fn a_typo_in_a_runtime_block_is_an_error() {
        assert!(
            serde_norway::from_str::<RuntimeSpec>("type: docker\ninterface: web\nprot: 8080\n")
                .is_err()
        );
    }
}
