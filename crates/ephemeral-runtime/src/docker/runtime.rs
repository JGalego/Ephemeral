//! Talking to the daemon.
//!
//! The only place in the crate that starts a process. Everything that
//! *decides* anything lives in [`super::command`], which is pure; what is
//! left here is spawning, reading output, and turning failures into messages
//! a person can act on ([ADR-0014]).
//!
//! [ADR-0014]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0014-drive-docker-through-its-cli.md

use std::process::{Command, Output};

use ephemeral_core::AppId;

use crate::{
    APP_LABEL, Availability, BuildRequest, Completed, ContainerState, ContainerStatus,
    ManagedContainer, Runtime, RuntimeError, Secrets, spec::ContainerSpec,
};

use super::command::{self, NetworkMode};

/// The name Ephemeral uses for this runtime.
const RUNTIME_NAME: &str = "Docker";

/// Runs generated applications in Docker containers.
///
/// Holds no state and no connection: each operation is one invocation of the
/// `docker` command. That is what lets Ephemeral inherit `DOCKER_HOST`, Docker
/// contexts, Docker Desktop's socket placement and Podman without implementing
/// any of it.
#[derive(Debug, Clone)]
pub struct DockerRuntime {
    program: String,
    user: Option<String>,
}

impl Default for DockerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerRuntime {
    /// A runtime driving the `docker` command from `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            program: "docker".to_owned(),
            user: host_identity(),
        }
    }

    /// A runtime driving some other compatible command, such as `podman`.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            user: host_identity(),
        }
    }

    /// Runs a `docker` subcommand and returns its output on success.
    ///
    /// `env` supplies values for `--env NAME` arguments. They are set on the
    /// child process rather than passed as arguments, so a secret value is never
    /// in an argument vector, an error message, or the process table.
    fn invoke(&self, args: &[String], env: &[(String, String)]) -> Result<Output, RuntimeError> {
        let mut process = Command::new(&self.program);
        process.args(args);
        for (name, value) in env {
            process.env(name, value);
        }

        process
            .output()
            .map_err(|source| RuntimeError::CommandUnavailable {
                command: self.describe(args),
                source,
            })
    }

    /// Runs a subcommand, requiring it to succeed.
    fn run_checked(
        &self,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<String, RuntimeError> {
        let output = self.invoke(args, env)?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }

        Err(RuntimeError::CommandFailed {
            command: self.describe(args),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }

    /// The command line, for an error message or the audit log.
    ///
    /// Safe to record verbatim, because no argument ever holds a secret value.
    fn describe(&self, args: &[String]) -> String {
        std::iter::once(self.program.as_str())
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The identity containers should run as, if a better one than `nobody` is
    /// known.
    ///
    /// Matching the invoking user's identity is what makes a granted writable
    /// mount actually writable: a bind mount carries the host's ownership, so a
    /// container running as `nobody` cannot write to a directory the user owns.
    #[must_use]
    pub fn container_user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Ensures the network an isolated-but-listening application needs exists.
    fn ensure_isolated_network(&self) -> Result<(), RuntimeError> {
        if self
            .invoke(&command::inspect_isolated_network(), &[])?
            .status
            .success()
        {
            return Ok(());
        }

        self.run_checked(&command::create_isolated_network(), &[])
            .map(|_| ())
            .map_err(|error| RuntimeError::CannotEnforce {
                control: "letting this app be reached without letting it reach out".to_owned(),
                reason: format!(
                    "Ephemeral could not create the isolated network it needs for that: {error}"
                ),
            })
    }

    /// The values for a specification's settings, checked to be present.
    fn environment_for(
        spec: &ContainerSpec,
        secrets: &Secrets,
    ) -> Result<Vec<(String, String)>, RuntimeError> {
        spec.environment_names
            .iter()
            .map(|name| match secrets.get(name) {
                Some(value) => Ok((name.clone(), value.to_owned())),
                None => Err(RuntimeError::CannotEnforce {
                    control: format!("giving this app the setting {name}"),
                    reason: format!(
                        "{name} was allowed, but no value for it is stored. Set it before \
                         running this app, or withdraw the permission."
                    ),
                }),
            })
            .collect()
    }
}

impl Runtime for DockerRuntime {
    fn name(&self) -> &'static str {
        RUNTIME_NAME
    }

    fn availability(&self) -> Availability {
        let args = command::version();

        let Ok(output) = self.invoke(&args, &[]) else {
            return Availability::unusable(format!(
                "{RUNTIME_NAME} does not appear to be installed — `{}` is not on your PATH. \
                 Install Docker Desktop, or Docker Engine on Linux, and run `ephemeral doctor` \
                 again.",
                self.program
            ));
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let hint = if stderr.contains("permission denied") {
                "Your user may not be in the `docker` group."
            } else {
                "Start Docker Desktop, or the Docker service, and try again."
            };

            return Availability::unusable(format!(
                "{RUNTIME_NAME} is installed but not responding. {hint} Until it is running, \
                 applications cannot be built or started."
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Availability::usable(format!("{RUNTIME_NAME} {version}"))
    }

    fn prepare_image(&self, image: &str) -> Result<(), RuntimeError> {
        self.run_checked(&command::pull(image), &[])
            .map(|_| ())
            .map_err(|error| RuntimeError::ImageUnavailable {
                image: image.to_owned(),
                reason: error.to_string(),
            })
    }

    fn build_image(&self, request: &BuildRequest) -> Result<String, RuntimeError> {
        let args = command::build(
            &request.app,
            &request.version,
            &request.context,
            &request.dockerfile,
        )?;

        let output = self.invoke(&args, &[])?;

        if output.status.success() {
            return Ok(command::image_tag(&request.app, &request.version));
        }

        // Both streams, in the order a person would have seen them. A build
        // failure's cause is as often on stdout as on stderr, and the whole of
        // it is what a repair attempt reads.
        let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
        printed.push_str(&String::from_utf8_lossy(&output.stderr));

        Err(RuntimeError::BuildFailed {
            app: request.app.clone(),
            summary: last_meaningful_line(&printed),
            output: printed,
        })
    }

    fn run_once(&self, spec: &ContainerSpec) -> Result<Completed, RuntimeError> {
        let mut spec = spec.clone();
        if let Some(user) = &self.user {
            spec.user.clone_from(user);
        }

        // Every refusal happens here, before anything runs.
        let args = command::run_once(&spec)?;

        if command::network_mode(&spec)? == NetworkMode::Isolated {
            self.ensure_isolated_network()?;
        }

        let output = self.invoke(&args, &[])?;
        let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
        printed.push_str(&String::from_utf8_lossy(&output.stderr));

        Ok(Completed {
            succeeded: output.status.success(),
            exit_code: output.status.code().unwrap_or(-1),
            output: printed,
        })
    }

    fn start(
        &self,
        spec: &ContainerSpec,
        secrets: &Secrets,
    ) -> Result<ContainerStatus, RuntimeError> {
        // Build the argument vector first. Every refusal this crate is capable
        // of happens here, before anything has been started or created.
        let mut spec = spec.clone();
        if let Some(user) = &self.user {
            spec.user.clone_from(user);
        }
        let args = command::run(&spec)?;
        let environment = Self::environment_for(&spec, secrets)?;

        let existing = self.status(&spec.app)?;
        if existing.state.is_live() {
            return Err(RuntimeError::AlreadyRunning {
                app: spec.app.clone(),
            });
        }
        if existing.state.exists() {
            // A stopped container still holds the name. Removing it is safe:
            // everything the application keeps lives in its data directory, not
            // in the container's writable layer, which is read-only anyway.
            self.remove(&spec.app)?;
        }

        if command::network_mode(&spec)? == NetworkMode::Isolated {
            self.ensure_isolated_network()?;
        }

        self.run_checked(&args, &environment)
            .map_err(|error| explain_isolation_failure(error, &spec))?;
        self.status(&spec.app)
    }

    fn stop(&self, app: &AppId) -> Result<(), RuntimeError> {
        self.run_checked(&command::stop(app), &[]).map(|_| ())
    }

    fn pause(&self, app: &AppId) -> Result<(), RuntimeError> {
        if !self.status(app)?.state.is_live() {
            return Err(RuntimeError::NotRunning { app: app.clone() });
        }
        self.run_checked(&command::pause(app), &[]).map(|_| ())
    }

    fn resume(&self, app: &AppId) -> Result<(), RuntimeError> {
        if self.status(app)?.state != ContainerState::Paused {
            return Err(RuntimeError::NotRunning { app: app.clone() });
        }
        self.run_checked(&command::resume(app), &[]).map(|_| ())
    }

    fn status(&self, app: &AppId) -> Result<ContainerStatus, RuntimeError> {
        let args = command::inspect(app);
        let output = self.invoke(&args, &[])?;

        // `inspect` failing is how Docker says "no such container", which is a
        // legitimate answer rather than an error.
        if !output.status.success() {
            return Ok(ContainerStatus::absent(app.clone()));
        }

        parse_inspect(app, &String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
            RuntimeError::UnreadableOutput {
                command: self.describe(&args),
                reason: "the container description was not in the expected shape".to_owned(),
            }
        })
    }

    fn logs(&self, app: &AppId, lines: u32) -> Result<String, RuntimeError> {
        let args = command::logs(app, lines);
        let output = self.invoke(&args, &[])?;

        if !output.status.success() {
            return Err(RuntimeError::CommandFailed {
                command: self.describe(&args),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }

        // Applications write to both streams and people want them interleaved,
        // which is what they would see had they run it themselves.
        let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(combined)
    }

    fn remove(&self, app: &AppId) -> Result<(), RuntimeError> {
        let args = command::remove(app);
        let output = self.invoke(&args, &[])?;

        // Removing what is not there is a success: teardown must be safe to
        // repeat, because it runs on paths that have already failed once.
        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") {
            return Ok(());
        }

        Err(RuntimeError::CommandFailed {
            command: self.describe(&args),
            status: output.status.to_string(),
            stderr: stderr.trim().to_owned(),
        })
    }

    fn managed_containers(&self) -> Result<Vec<ManagedContainer>, RuntimeError> {
        let listing = self.run_checked(&command::list_managed(), &[])?;
        Ok(parse_container_listing(&listing))
    }
}

/// Recognises the one confinement Ephemeral cannot verify without a daemon.
///
/// An application that listens but may not call out is put on an `--internal`
/// network so it is reachable from this machine and unable to reach off it.
/// Whether Docker will publish a port on such a network is not something the
/// argument-vector tests can establish, so if it refuses, the failure is named
/// rather than handed over as a raw daemon error.
///
/// The result is still a refusal. If this combination turns out to be
/// unsupported, an application that listens does not run — it does not quietly
/// get ordinary networking instead.
fn explain_isolation_failure(error: RuntimeError, spec: &ContainerSpec) -> RuntimeError {
    let RuntimeError::CommandFailed { stderr, .. } = &error else {
        return error;
    };

    let about_the_network = stderr.contains("conflicting options")
        || (stderr.contains("port") && stderr.contains("network"));

    if !about_the_network || spec.ports.is_empty() {
        return error;
    }

    RuntimeError::CannotEnforce {
        control: "letting this app be reached without letting it reach out".to_owned(),
        reason: format!(
            "Docker refused to publish a port on an internal network: {stderr}. Ephemeral \
             will not fall back to ordinary networking, because that would give this \
             application the whole internet when its owner allowed none of it. Granting it \
             outbound access explicitly would let it run."
        ),
    }
}

/// The line worth showing a person from a wall of build output.
///
/// `BuildKit` prefixes *every* line with a step marker like `#5` or `#5 2.431`,
/// including the error, so those markers cannot be used to tell noise from
/// signal — they are stripped rather than filtered on. The last line mentioning
/// an error wins; failing that, the last line of any kind.
///
/// This is a heuristic and only decides the one-line message. The full output is
/// always kept, because that is what a repair attempt reads.
fn last_meaningful_line(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let chosen = lines
        .iter()
        .rfind(|line| {
            let lowered = line.to_lowercase();
            lowered.contains("error") || lowered.contains("failed")
        })
        .or_else(|| lines.last());

    chosen.map_or_else(
        || "the build produced no output".to_owned(),
        |line| strip_step_marker(line),
    )
}

/// Removes `BuildKit`'s `#5` or `#5 2.431` prefix from one line.
fn strip_step_marker(line: &str) -> String {
    let Some(rest) = line.strip_prefix('#') else {
        return line.to_owned();
    };

    // `#5 2.431 ERROR: ...` — drop the step number, then a bare timestamp if
    // one follows. Anything that does not match is left exactly as it was.
    let mut parts = rest.splitn(3, ' ');
    let (Some(step), Some(second)) = (parts.next(), parts.next()) else {
        return line.to_owned();
    };
    if !step.chars().all(|c| c.is_ascii_digit()) {
        return line.to_owned();
    }

    let looks_like_a_timestamp =
        second.chars().all(|c| c.is_ascii_digit() || c == '.') && second.contains('.');

    if looks_like_a_timestamp {
        parts.next().unwrap_or(second).to_owned()
    } else {
        rest[step.len()..].trim_start().to_owned()
    }
}

/// Reads the parts of `docker inspect` Ephemeral relies on.
///
/// Deliberately partial. Docker's inspect output is large and version-dependent,
/// and a struct that insisted on all of it would break on an upgrade that
/// changed a field nothing here reads.
fn parse_inspect(app: &AppId, json: &str) -> Option<ContainerStatus> {
    let containers: serde_json::Value = serde_json::from_str(json).ok()?;
    let container = containers.get(0)?;
    let state = container.get("State")?;

    Some(ContainerStatus {
        app: app.clone(),
        state: ContainerState::from_docker_status(state.get("Status")?.as_str()?),
        container_id: container
            .get("Id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        exit_code: state.get("ExitCode").and_then(serde_json::Value::as_i64),
        health: state
            .get("Health")
            .and_then(|health| health.get("Status"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
    })
}

/// Reads `docker ps --format '{{json .}}'`, which emits one object per line.
///
/// A line that cannot be read is skipped rather than failing the listing: this
/// feeds orphan cleanup, and one unreadable entry should not stop the others
/// being tidied up.
fn parse_container_listing(output: &str) -> Vec<ManagedContainer> {
    output
        .lines()
        .filter_map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            let name = entry.get("Names")?.as_str()?.split(',').next()?.to_owned();

            Some(ManagedContainer {
                name,
                app: entry
                    .get("Labels")
                    .and_then(serde_json::Value::as_str)
                    .and_then(app_from_labels),
                state: entry
                    .get("State")
                    .and_then(serde_json::Value::as_str)
                    .map_or(ContainerState::Dead, ContainerState::from_docker_status),
            })
        })
        .collect()
}

/// Pulls the application id out of Docker's comma-separated label string.
fn app_from_labels(labels: &str) -> Option<AppId> {
    labels
        .split(',')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == APP_LABEL)
        .and_then(|(_, value)| AppId::parse(value).ok())
}

/// The invoking user's `uid:gid`, where the platform has such a thing.
///
/// Asked of `id` rather than read through a system call, because this crate
/// forbids unsafe code and will not take a dependency on a libc binding to
/// answer one question.
#[cfg(unix)]
fn host_identity() -> Option<String> {
    let ask = |flag: &str| {
        let output = Command::new("id").arg(flag).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
    };

    let uid = ask("-u")?;
    let gid = ask("-g")?;

    // Running as root on the host is not a reason to run as root in the
    // container. Fall back to `nobody`, which is what the specification already
    // carries.
    if uid == "0" {
        return None;
    }

    Some(format!("{uid}:{gid}"))
}

/// Windows containers have no equivalent identity to borrow.
#[cfg(not(unix))]
fn host_identity() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppId {
        AppId::parse("csv-comparator").unwrap()
    }

    /// The one-line message should be the error, with `BuildKit`'s step marker
    /// out of the way. Every line of that output starts with `#`, which is why
    /// the marker cannot be used to tell noise from signal.
    /// This is the one confinement no test here can establish, so the failure
    /// it would produce is at least legible.
    #[test]
    fn a_refused_port_on_an_internal_network_is_named_rather_than_relayed() {
        let mut spec = ContainerSpec::minimal(app(), "alpine", vec![]);
        spec.ports = vec![crate::PortBinding::loopback(8080, 8080)];

        let raw = RuntimeError::CommandFailed {
            command: "docker run ...".to_owned(),
            status: "exit status 125".to_owned(),
            stderr: "conflicting options: port publishing and the container type network mode"
                .to_owned(),
        };

        let explained = explain_isolation_failure(raw, &spec);
        let RuntimeError::CannotEnforce { reason, .. } = &explained else {
            panic!("expected a refusal, got {explained:?}");
        };

        assert!(reason.contains("will not fall back"), "{reason}");
        assert!(reason.contains("whole internet"), "{reason}");
    }

    /// An unrelated failure must not be dressed up as this one.
    #[test]
    fn an_unrelated_failure_is_left_alone() {
        let mut spec = ContainerSpec::minimal(app(), "alpine", vec![]);
        spec.ports = vec![crate::PortBinding::loopback(8080, 8080)];

        let raw = RuntimeError::CommandFailed {
            command: "docker run ...".to_owned(),
            status: "exit status 125".to_owned(),
            stderr: "no such image: alpine".to_owned(),
        };

        assert!(matches!(
            explain_isolation_failure(raw, &spec),
            RuntimeError::CommandFailed { .. }
        ));
    }

    #[test]
    fn a_build_failure_is_summarised_by_its_error() {
        let output = concat!(
            "#5 [3/4] RUN pip install -r requirements.txt\n",
            "#5 2.431 ERROR: no version of pandas matches ==99.0\n",
            "#5 ERROR: process did not complete successfully\n",
            "\n",
        );

        let summary = last_meaningful_line(output);
        assert!(summary.contains("did not complete"), "{summary}");
        assert!(!summary.starts_with('#'), "{summary}");
    }

    /// When nothing announces itself as an error, the last line is still more
    /// useful than nothing.
    #[test]
    fn output_with_no_error_line_falls_back_to_the_last_line() {
        let summary = last_meaningful_line("#1 building\n#2 something odd happened\n");
        assert!(summary.contains("something odd"), "{summary}");
    }

    #[test]
    fn step_markers_are_stripped_only_when_they_are_step_markers() {
        assert_eq!(strip_step_marker("#5 2.431 ERROR: boom"), "ERROR: boom");
        assert_eq!(strip_step_marker("#5 ERROR: boom"), "ERROR: boom");
        assert_eq!(strip_step_marker("ERROR: boom"), "ERROR: boom");
        assert_eq!(
            strip_step_marker("#include <stdio.h> failed"),
            "#include <stdio.h> failed"
        );
    }

    #[test]
    fn empty_build_output_still_says_something() {
        assert!(!last_meaningful_line("").is_empty());
        assert!(!last_meaningful_line("\n\n  \n").is_empty());
    }

    #[test]
    fn a_running_container_is_read_from_inspect_output() {
        let json = r#"[{
            "Id": "abc123",
            "State": { "Status": "running", "ExitCode": 0 }
        }]"#;

        let status = parse_inspect(&app(), json).unwrap();
        assert_eq!(status.state, ContainerState::Running);
        assert_eq!(status.container_id.as_deref(), Some("abc123"));
        assert_eq!(status.health, None);
    }

    #[test]
    fn a_failing_health_check_is_read_and_reported() {
        let json = r#"[{
            "Id": "abc123",
            "State": { "Status": "running", "ExitCode": 0, "Health": { "Status": "unhealthy" } }
        }]"#;

        let status = parse_inspect(&app(), json).unwrap();
        assert!(status.is_unhealthy());
    }

    /// Docker's inspect output is large and changes between versions. Reading
    /// only what is needed means an added field is not an outage.
    #[test]
    fn unknown_fields_in_inspect_output_are_ignored() {
        let json = r#"[{
            "Id": "abc123",
            "SomethingAddedInAFutureVersion": { "nested": [1, 2, 3] },
            "State": { "Status": "exited", "ExitCode": 137, "Whatever": true }
        }]"#;

        let status = parse_inspect(&app(), json).unwrap();
        assert_eq!(status.exit_code, Some(137));
        assert!(status.is_unhealthy());
    }

    #[test]
    fn output_that_cannot_be_understood_is_not_guessed_at() {
        assert!(parse_inspect(&app(), "[]").is_none());
        assert!(parse_inspect(&app(), "not json").is_none());
        assert!(parse_inspect(&app(), r#"[{"Id":"x"}]"#).is_none());
    }

    #[test]
    fn managed_containers_are_read_from_the_listing() {
        let listing = concat!(
            r#"{"ID":"a1","Names":"ephemeral-csv-comparator","State":"running","#,
            r#""Labels":"sh.ephemeral.managed=true,sh.ephemeral.app=csv-comparator"}"#,
            "\n",
            r#"{"ID":"b2","Names":"ephemeral-note-taker","State":"exited","#,
            r#""Labels":"sh.ephemeral.app=note-taker"}"#,
        );

        let containers = parse_container_listing(listing);
        assert_eq!(containers.len(), 2);
        assert_eq!(containers[0].app, Some(app()));
        assert_eq!(containers[0].state, ContainerState::Running);
        assert_eq!(containers[1].state, ContainerState::Exited);
    }

    /// Orphan cleanup runs after something already went wrong, so one
    /// unreadable entry must not stop the rest being tidied up.
    #[test]
    fn an_unreadable_listing_entry_does_not_hide_the_others() {
        let listing = concat!(
            "garbage that is not json\n",
            r#"{"ID":"b2","Names":"ephemeral-note-taker","State":"exited","Labels":""}"#,
        );

        assert_eq!(parse_container_listing(listing).len(), 1);
    }

    /// A label that is not a valid application id must not become one.
    #[test]
    fn a_malformed_label_yields_no_application() {
        assert_eq!(app_from_labels("sh.ephemeral.app=Not A Valid Id"), None);
        assert_eq!(app_from_labels("sh.ephemeral.managed=true"), None);
        assert_eq!(
            app_from_labels("sh.ephemeral.managed=true,sh.ephemeral.app=note-taker"),
            Some(AppId::parse("note-taker").unwrap())
        );
    }

    /// Running Ephemeral as root is not a reason to run generated code as root.
    #[test]
    fn the_container_identity_is_never_root() {
        let runtime = DockerRuntime::new();
        if let Some(user) = runtime.container_user() {
            assert!(!user.starts_with("0:"), "{user}");
        }
    }

    /// The command line goes into error messages and the audit log, so it has
    /// to be the real one and it has to be free of secrets.
    #[test]
    fn described_commands_are_verbatim_and_secret_free() {
        let runtime = DockerRuntime::new();
        let described = runtime.describe(&command::logs(&app(), 50));

        assert_eq!(described, "docker logs --tail 50 ephemeral-csv-comparator");
    }

    #[test]
    fn a_missing_setting_value_is_a_refusal_with_a_remedy() {
        let mut spec = ContainerSpec::minimal(app(), "alpine", vec![]);
        spec.environment_names = vec!["API_KEY".to_owned()];

        let error = DockerRuntime::environment_for(&spec, &Secrets::new()).unwrap_err();
        let message = error.to_string();

        assert!(message.contains("API_KEY"), "{message}");
        assert!(message.contains("no value"), "{message}");
    }

    #[test]
    fn a_present_setting_value_is_passed_through_the_environment() {
        let mut spec = ContainerSpec::minimal(app(), "alpine", vec![]);
        spec.environment_names = vec!["API_KEY".to_owned()];

        let mut secrets = Secrets::new();
        secrets.insert("API_KEY", "sk-live-value");

        let environment = DockerRuntime::environment_for(&spec, &secrets).unwrap();
        assert_eq!(
            environment,
            vec![("API_KEY".to_owned(), "sk-live-value".to_owned())]
        );

        // And not into the arguments, which is the property that matters.
        let args = command::run(&spec).unwrap();
        assert!(!args.iter().any(|arg| arg.contains("sk-live")), "{args:?}");
    }
}
