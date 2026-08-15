//! The argument vectors Ephemeral hands to `docker`.
//!
//! Every function here is pure: a [`ContainerSpec`] in, a `Vec<String>` out.
//! That is the whole point of [ADR-0014]. The confinement a generated
//! application runs under is decided by these functions, so the confinement can
//! be asserted in unit tests that run everywhere — including in CI, where no
//! container daemon exists and no container is ever started.
//!
//! Two rules hold throughout:
//!
//! - **Nothing is passed through a shell.** These are argument vectors, not
//!   command lines. The values in them include user-chosen paths and
//!   model-generated strings, and there is no quoting problem here because there
//!   is nothing to quote for.
//! - **No secret value ever appears in an argument.** Settings are passed as
//!   `--env NAME` with no value, and the value is supplied through the child
//!   process's own environment. An argument vector can therefore be written
//!   verbatim into the audit log, which is what makes the log worth reading.
//!
//! [ADR-0014]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0014-drive-docker-through-its-cli.md

use ephemeral_core::{AppId, permission::HostScope};

use crate::{
    APP_LABEL, CONTAINER_PREFIX, Egress, MANAGED_LABEL, RuntimeError, spec::ContainerSpec,
};

/// How long Docker waits for an application to shut down before killing it.
const STOP_TIMEOUT_SECONDS: u32 = 10;

/// The writable scratch space every container gets in place of a writable root.
///
/// `noexec` and `nosuid` because a temporary directory is where a compromised
/// process would most naturally try to drop something to run.
const TMPFS: &str = "/tmp:rw,noexec,nosuid,nodev,size=64m";

/// The container name for an application.
#[must_use]
pub fn container_name(app: &AppId) -> String {
    format!("{CONTAINER_PREFIX}{app}")
}

/// The arguments that start an application under confinement.
///
/// # Errors
///
/// [`RuntimeError::CannotEnforce`] when the specification asks for a control
/// Docker cannot apply — an outbound allow-list, or a path Docker's mount syntax
/// cannot express unambiguously. The application is not started in either case.
pub fn run(spec: &ContainerSpec) -> Result<Vec<String>, RuntimeError> {
    let mut args = vec![
        "run".to_owned(),
        "--detach".to_owned(),
        "--name".to_owned(),
        spec.container_name(),
        "--label".to_owned(),
        format!("{MANAGED_LABEL}=true"),
        "--label".to_owned(),
        format!("{APP_LABEL}={}", spec.app),
    ];

    args.extend(hardening(spec));
    args.extend(resource_limits(spec));
    args.extend(network(spec)?);
    args.extend(mounts(spec)?);

    // Names only. The values are set on the `docker` process's environment, so
    // they are inherited by the container without ever being an argument.
    for name in &spec.environment_names {
        args.push("--env".to_owned());
        args.push(name.clone());
    }

    args.push("--workdir".to_owned());
    args.push(spec.working_dir.clone());

    args.push(spec.image.clone());
    args.extend(spec.entrypoint.iter().cloned());

    Ok(args)
}

/// The flags that hold, whatever the application was granted.
///
/// None of these is derived from a permission, because none of them is
/// negotiable. An application that needs root, or a writable root filesystem, or
/// the ability to gain privileges, is an application Ephemeral does not run.
fn hardening(spec: &ContainerSpec) -> Vec<String> {
    vec![
        // Every Linux capability dropped. A generated application has no reason
        // to bind a low port, change ownership, or load a module.
        "--cap-drop=ALL".to_owned(),
        // No path from inside to more privilege than it started with, which is
        // what makes dropping capabilities durable rather than advisory.
        "--security-opt=no-new-privileges".to_owned(),
        // The image is read-only; the only writable places are the tmpfs below
        // and the mounts a person granted.
        "--read-only".to_owned(),
        "--tmpfs".to_owned(),
        TMPFS.to_owned(),
        // Not root, ever.
        "--user".to_owned(),
        spec.user.clone(),
        // Ephemeral decides when something restarts, based on the lifecycle
        // state machine. A container that resurrects itself after the state
        // machine stopped it would be a state machine that is not in charge.
        "--restart".to_owned(),
        "no".to_owned(),
        "--stop-timeout".to_owned(),
        STOP_TIMEOUT_SECONDS.to_string(),
    ]
}

/// The ceilings on what the application may consume.
///
/// The wall-clock limit and the disk ceiling are absent deliberately: Docker
/// enforces neither in a way that covers what Ephemeral promises. The supervisor
/// applies the first by stopping the container, and the second over the
/// application's data directory on the host, which is where its storage actually
/// lives.
fn resource_limits(spec: &ContainerSpec) -> Vec<String> {
    let memory = format!("{}m", spec.limits.memory_mib);

    vec![
        "--memory".to_owned(),
        memory.clone(),
        // Equal to the memory limit, which is how Docker is told "no swap".
        // Without it a memory-limited container simply swaps instead, and the
        // ceiling the user was shown is not the ceiling that applies.
        "--memory-swap".to_owned(),
        memory,
        "--cpus".to_owned(),
        format!("{:.3}", f64::from(spec.limits.cpu_millis) / 1000.0),
        // The limit that bounds a fork bomb, deliberate or otherwise.
        "--pids-limit".to_owned(),
        spec.limits.max_processes.to_string(),
    ]
}

/// The network a container is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// No network interfaces at all. The default, and the strongest.
    None,

    /// A network Ephemeral creates with `--internal`: reachable from this
    /// machine, unable to reach anything off it.
    ///
    /// Needed because Docker refuses to publish a port on a container with no
    /// network at all, and an application that listens is not thereby an
    /// application allowed to call out.
    Isolated,

    /// Ordinary outbound networking. Only when the user granted it.
    Bridge,
}

impl NetworkMode {
    /// The value for `--network`.
    #[must_use]
    pub fn as_argument(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Isolated => ISOLATED_NETWORK,
            Self::Bridge => "bridge",
        }
    }

    /// Whether the application can reach anything beyond this machine.
    #[must_use]
    pub fn permits_egress(self) -> bool {
        matches!(self, Self::Bridge)
    }
}

/// The network Ephemeral creates for applications that listen but may not call
/// out.
pub const ISOLATED_NETWORK: &str = "ephemeral-isolated";

/// Which network a specification calls for.
///
/// # Errors
///
/// [`RuntimeError::CannotEnforce`] for an outbound allow-list, which Docker has
/// no way to apply.
pub fn network_mode(spec: &ContainerSpec) -> Result<NetworkMode, RuntimeError> {
    match &spec.egress {
        // The honest failure. Docker has no per-destination egress filter, and
        // `bridge` would give the whole internet to an application whose owner
        // allowed four hostnames. Refusing is the only answer that does not
        // quietly grant more than was granted.
        Egress::AllowList(hosts) => {
            let named: Vec<String> = hosts.iter().map(HostScope::as_written).collect();
            Err(RuntimeError::CannotEnforce {
                control: format!("network access limited to {}", named.join(", ")),
                reason: "Docker cannot filter outbound traffic by destination, and running this \
                         application with ordinary networking would give it the whole internet \
                         instead of the sites you allowed. Honouring this needs a filtering \
                         proxy, which Ephemeral does not have yet."
                    .to_owned(),
            })
        }
        Egress::Anywhere => Ok(NetworkMode::Bridge),
        Egress::Denied if spec.ports.is_empty() => Ok(NetworkMode::None),
        Egress::Denied => Ok(NetworkMode::Isolated),
    }
}

/// The network the application gets, and the ports it publishes.
fn network(spec: &ContainerSpec) -> Result<Vec<String>, RuntimeError> {
    let mut args = vec![
        "--network".to_owned(),
        network_mode(spec)?.as_argument().to_owned(),
    ];

    for port in &spec.ports {
        args.push("--publish".to_owned());
        args.push(format!(
            "{}:{}:{}",
            port.host_address(),
            port.host_port,
            port.container_port
        ));
    }

    Ok(args)
}

/// The arguments that create the network used by applications that listen but
/// may not call out.
///
/// `--internal` is what makes it a confinement rather than a convenience: Docker
/// installs rules that stop the network reaching anything outside itself.
#[must_use]
pub fn create_isolated_network() -> Vec<String> {
    vec![
        "network".to_owned(),
        "create".to_owned(),
        "--internal".to_owned(),
        "--label".to_owned(),
        format!("{MANAGED_LABEL}=true"),
        ISOLATED_NETWORK.to_owned(),
    ]
}

/// The arguments that ask whether the isolated network already exists.
#[must_use]
pub fn inspect_isolated_network() -> Vec<String> {
    vec![
        "network".to_owned(),
        "inspect".to_owned(),
        ISOLATED_NETWORK.to_owned(),
    ]
}

/// The host directories the application can see.
fn mounts(spec: &ContainerSpec) -> Result<Vec<String>, RuntimeError> {
    let mut args = Vec::new();

    for mount in &spec.mounts {
        let source = mount.host_path.to_string_lossy().into_owned();

        // Docker's `--mount` takes comma-separated key=value pairs, so a path
        // containing a comma or a quote would be read as several options. That
        // is close enough to an injection to treat it as one: refuse rather than
        // hand over something whose meaning is not the one intended.
        if source.contains(',') || source.contains('"') {
            return Err(RuntimeError::CannotEnforce {
                control: format!("access to {source}"),
                reason: "Docker's mount syntax cannot express a path containing a comma or a \
                         quotation mark unambiguously, so Ephemeral will not attempt it"
                    .to_owned(),
            });
        }

        let mut option = format!("type=bind,source={source},target={}", mount.container_path);
        if !mount.writable {
            option.push_str(",readonly");
        }

        args.push("--mount".to_owned());
        args.push(option);
    }

    Ok(args)
}

/// The arguments that fetch an image.
#[must_use]
pub fn pull(image: &str) -> Vec<String> {
    vec!["pull".to_owned(), image.to_owned()]
}

/// The arguments that ask an application to stop.
#[must_use]
pub fn stop(app: &AppId) -> Vec<String> {
    vec![
        "stop".to_owned(),
        "--timeout".to_owned(),
        STOP_TIMEOUT_SECONDS.to_string(),
        container_name(app),
    ]
}

/// The arguments that suspend a running application.
#[must_use]
pub fn pause(app: &AppId) -> Vec<String> {
    vec!["pause".to_owned(), container_name(app)]
}

/// The arguments that resume a suspended application.
#[must_use]
pub fn resume(app: &AppId) -> Vec<String> {
    vec!["unpause".to_owned(), container_name(app)]
}

/// The arguments that remove an application's container and its anonymous
/// volumes.
#[must_use]
pub fn remove(app: &AppId) -> Vec<String> {
    vec![
        "rm".to_owned(),
        "--force".to_owned(),
        // Anything the container wrote outside a granted mount goes with it.
        // Leaving it behind would make "deleted" mean "mostly deleted".
        "--volumes".to_owned(),
        container_name(app),
    ]
}

/// The arguments that ask what an application's container is doing.
#[must_use]
pub fn inspect(app: &AppId) -> Vec<String> {
    vec![
        "inspect".to_owned(),
        "--type".to_owned(),
        "container".to_owned(),
        container_name(app),
    ]
}

/// The arguments that read an application's recent output.
#[must_use]
pub fn logs(app: &AppId, lines: u32) -> Vec<String> {
    vec![
        "logs".to_owned(),
        "--tail".to_owned(),
        lines.to_string(),
        container_name(app),
    ]
}

/// The arguments that list every container Ephemeral created.
///
/// Filtered by Ephemeral's own label rather than by name, and including stopped
/// containers, because orphan cleanup must find what a crash left behind without
/// touching anything else the user is running.
#[must_use]
pub fn list_managed() -> Vec<String> {
    vec![
        "ps".to_owned(),
        "--all".to_owned(),
        "--filter".to_owned(),
        format!("label={MANAGED_LABEL}=true"),
        "--format".to_owned(),
        "{{json .}}".to_owned(),
    ]
}

/// The arguments that ask the daemon whether it is there.
#[must_use]
pub fn version() -> Vec<String> {
    vec![
        "version".to_owned(),
        "--format".to_owned(),
        "{{.Server.Version}}".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ephemeral_core::{
        manifest::ResourceLimits,
        permission::{AppPermission, HostScope, PathScope},
    };

    use super::*;
    use crate::spec::{ContainerSpec, HostPaths, PortBinding};

    fn app() -> AppId {
        AppId::parse("csv-comparator").unwrap()
    }

    fn paths() -> HostPaths {
        HostPaths {
            home: PathBuf::from("/home/ana"),
            data_dir: PathBuf::from("/home/ana/.local/share/ephemeral/apps/csv-comparator/data"),
        }
    }

    fn spec_with(granted: &[AppPermission]) -> ContainerSpec {
        ContainerSpec::from_grants(
            app(),
            "python:3.12-slim",
            vec!["python".to_owned(), "main.py".to_owned()],
            ResourceLimits::default(),
            granted,
            &paths(),
        )
        .unwrap()
    }

    /// Asserts a flag and its value appear next to each other, which
    /// `contains` on a flattened string would not.
    #[track_caller]
    fn assert_pair(args: &[String], flag: &str, value: &str) {
        let found = args
            .windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value);
        assert!(found, "expected `{flag} {value}` in {args:?}");
    }

    /// A path under the test home, rendered the way this host renders paths.
    ///
    /// A mount source is a *host* path, so its separators are the host's. An
    /// assertion written with forward slashes would pass on Linux and fail on
    /// Windows for no reason connected to what is being tested.
    fn under_home(relative: &str) -> String {
        relative
            .split('/')
            .fold(PathBuf::from("/home/ana"), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned()
    }

    #[track_caller]
    fn assert_flag(args: &[String], flag: &str) {
        assert!(
            args.iter().any(|arg| arg == flag),
            "expected `{flag}` in {args:?}"
        );
    }

    /// The controls that are not negotiable. If any of these stops being
    /// emitted, generated code is running with more of the machine than
    /// Ephemeral says it is.
    #[test]
    fn every_container_is_hardened_whatever_it_was_granted() {
        let args = run(&spec_with(&[])).unwrap();

        assert_flag(&args, "--cap-drop=ALL");
        assert_flag(&args, "--security-opt=no-new-privileges");
        assert_flag(&args, "--read-only");
        assert_pair(&args, "--tmpfs", TMPFS);
        assert_pair(&args, "--restart", "no");
    }

    /// Nothing generated runs as root, and nothing here has a path that
    /// produces uid 0.
    #[test]
    fn nothing_runs_as_root() {
        let args = run(&spec_with(&[])).unwrap();

        let user = args
            .windows(2)
            .find(|pair| pair[0] == "--user")
            .map(|pair| pair[1].clone())
            .expect("a container must be given an identity to run as");

        assert_ne!(user, "0:0");
        assert_ne!(user, "root");
        assert!(!user.starts_with("0:"), "{user} is root");
    }

    /// Denied is the default and the shape of denial is `--network none`, not
    /// an omitted flag. An omitted flag would mean Docker's default bridge.
    #[test]
    fn no_network_grant_means_no_network() {
        let args = run(&spec_with(&[])).unwrap();
        assert_pair(&args, "--network", "none");
        assert!(!args.iter().any(|arg| arg == "bridge"));
    }

    /// The whole internet is reachable only when the user granted exactly that.
    #[test]
    fn the_network_opens_only_when_anywhere_was_granted() {
        let anywhere = spec_with(&[AppPermission::outbound(HostScope::parse("*").unwrap())]);
        let args = run(&anywhere).unwrap();
        assert_pair(&args, "--network", "bridge");
    }

    /// A control Ephemeral cannot apply is a refusal, never a quiet
    /// substitution of a weaker one. Giving an app the whole internet because
    /// its owner allowed four hostnames would be exactly that.
    #[test]
    fn an_unenforceable_allow_list_refuses_rather_than_opening_the_network() {
        let limited = spec_with(&[AppPermission::outbound(
            HostScope::parse("api.example.com").unwrap(),
        )]);

        let error = run(&limited).unwrap_err();
        let RuntimeError::CannotEnforce { control, reason } = &error else {
            panic!("expected a refusal, got {error:?}");
        };

        assert!(control.contains("api.example.com"), "{control}");
        assert!(reason.contains("filter"), "{reason}");
    }

    /// A published port reaches this machine and nothing else, unless somebody
    /// decided otherwise.
    #[test]
    fn ports_bind_to_loopback() {
        let listening = spec_with(&[AppPermission::NetworkInbound { port: 8080 }]);
        let args = run(&listening).unwrap();

        assert_pair(&args, "--publish", "127.0.0.1:8080:8080");
        assert!(
            !args.iter().any(|arg| arg.starts_with("0.0.0.0")),
            "{args:?}"
        );
    }

    /// Listening is not calling out. An application granted an inbound port and
    /// nothing else gets a network it cannot leave.
    #[test]
    fn an_app_that_listens_still_cannot_reach_the_internet() {
        let listening = spec_with(&[AppPermission::NetworkInbound { port: 8080 }]);

        let mode = network_mode(&listening).unwrap();
        assert_eq!(mode, NetworkMode::Isolated);
        assert!(!mode.permits_egress());

        let args = run(&listening).unwrap();
        assert_pair(&args, "--network", ISOLATED_NETWORK);
        assert!(!args.iter().any(|arg| arg == "bridge"), "{args:?}");
    }

    /// The flag that makes the isolated network a confinement rather than a
    /// name.
    #[test]
    fn the_isolated_network_is_created_internal() {
        assert_flag(&create_isolated_network(), "--internal");
        assert!(
            create_isolated_network()
                .iter()
                .any(|arg| arg.contains(MANAGED_LABEL)),
            "cleanup has to be able to find it"
        );
    }

    #[test]
    fn publishing_beyond_loopback_takes_a_deliberate_decision() {
        let mut spec = spec_with(&[AppPermission::NetworkInbound { port: 8080 }]);
        spec.ports = vec![PortBinding {
            container_port: 8080,
            host_port: 8080,
            publicly_reachable: true,
        }];

        let args = run(&spec).unwrap();
        assert_pair(&args, "--publish", "0.0.0.0:8080:8080");
    }

    /// A read grant must not become a writable mount at any point in the chain
    /// from the ledger to the command line.
    #[test]
    fn a_read_grant_mounts_read_only() {
        let reader = spec_with(&[AppPermission::read(
            PathScope::parse("~/Downloads/apartments/**").unwrap(),
        )]);
        let args = run(&reader).unwrap();
        let granted = under_home("Downloads/apartments");

        let mount = args
            .windows(2)
            .find(|pair| pair[0] == "--mount" && pair[1].contains(&granted))
            .map(|pair| pair[1].clone())
            .expect("the granted directory should be mounted");

        assert!(mount.contains("type=bind"), "{mount}");
        assert!(mount.ends_with(",readonly"), "{mount}");
    }

    #[test]
    fn a_write_grant_mounts_writable() {
        let writer = spec_with(&[AppPermission::write(
            PathScope::parse("~/Reports/**").unwrap(),
        )]);
        let args = run(&writer).unwrap();
        let granted = under_home("Reports");

        let mount = args
            .windows(2)
            .find(|pair| pair[0] == "--mount" && pair[1].contains(&granted))
            .map(|pair| pair[1].clone())
            .expect("the granted directory should be mounted");

        assert!(!mount.contains("readonly"), "{mount}");
    }

    /// An application with no filesystem grant sees its own storage and
    /// nothing else of the user's.
    #[test]
    fn an_ungranted_app_mounts_only_its_own_storage() {
        let args = run(&spec_with(&[])).unwrap();

        let sources: Vec<&String> = args
            .windows(2)
            .filter(|pair| pair[0] == "--mount")
            .map(|pair| &pair[1])
            .collect();

        assert_eq!(sources.len(), 1, "{sources:?}");
        assert!(sources[0].contains("target=/data"), "{}", sources[0]);
    }

    /// Every ceiling the user was shown has to reach the daemon. A limit that
    /// is displayed and not applied is worse than no limit.
    #[test]
    fn every_resource_ceiling_is_passed_through() {
        let args = run(&spec_with(&[])).unwrap();
        let limits = ResourceLimits::default();

        assert_pair(&args, "--memory", &format!("{}m", limits.memory_mib));
        assert_pair(&args, "--cpus", "0.500");
        assert_pair(&args, "--pids-limit", &limits.max_processes.to_string());
    }

    /// Without this, a memory-limited container swaps instead of stopping, and
    /// the ceiling the user was shown is not the one that applies.
    #[test]
    fn the_memory_limit_is_not_quietly_a_swap_limit() {
        let args = run(&spec_with(&[])).unwrap();
        let limits = ResourceLimits::default();
        let expected = format!("{}m", limits.memory_mib);

        assert_pair(&args, "--memory-swap", &expected);
    }

    /// The property that lets an argument vector be written into the audit log
    /// verbatim.
    #[test]
    fn secret_values_never_enter_the_argument_vector() {
        let with_secret = spec_with(&[AppPermission::ReadEnvironment {
            name: "API_KEY".to_owned(),
        }]);
        let args = run(&with_secret).unwrap();

        assert_pair(&args, "--env", "API_KEY");
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains('=') && arg.contains("API_KEY")),
            "a name=value pair would put the value in the process table: {args:?}"
        );
    }

    /// A generated entrypoint is data, not a command line. Nothing here is
    /// handed to a shell, so a semicolon in an argument is a semicolon.
    #[test]
    fn the_entrypoint_is_never_reparsed() {
        let mut spec = spec_with(&[]);
        spec.entrypoint = vec![
            "python".to_owned(),
            "-c".to_owned(),
            "x; rm -rf /".to_owned(),
        ];

        let args = run(&spec).unwrap();

        assert_eq!(args.last().unwrap(), "x; rm -rf /");
        assert!(
            !args
                .iter()
                .any(|arg| arg == "sh" || arg == "-c" && args[0] == "sh")
        );
    }

    /// The image and the entrypoint come last, so that nothing after the image
    /// can be read as an option to `docker run`.
    #[test]
    fn the_image_precedes_the_entrypoint_and_all_options_precede_both() {
        let args = run(&spec_with(&[])).unwrap();
        let image = args
            .iter()
            .position(|arg| arg == "python:3.12-slim")
            .unwrap();

        assert!(args[..image].iter().any(|arg| arg == "--cap-drop=ALL"));
        assert_eq!(args[image + 1], "python");
        assert!(
            !args[image + 1..].iter().any(|arg| arg.starts_with("--")),
            "an option after the image would be an argument to the application"
        );
    }

    /// Docker's mount option list is comma-separated, so a path containing a
    /// comma would silently become several options.
    #[test]
    fn a_path_docker_cannot_express_is_refused() {
        let mut spec = spec_with(&[]);
        spec.mounts[0].host_path = PathBuf::from("/home/ana/Reports, drafts");

        let error = run(&spec).unwrap_err();
        assert!(
            matches!(error, RuntimeError::CannotEnforce { .. }),
            "{error:?}"
        );
    }

    /// Cleanup addresses containers by Ephemeral's own prefix and label, so it
    /// can never reap something the user started themselves.
    #[test]
    fn every_container_command_is_namespaced_to_ephemeral() {
        for args in [
            stop(&app()),
            pause(&app()),
            resume(&app()),
            remove(&app()),
            inspect(&app()),
            logs(&app(), 100),
        ] {
            assert!(
                args.iter().any(|arg| arg == &container_name(&app())),
                "{args:?}"
            );
            assert!(
                args.iter().any(|arg| arg.starts_with(CONTAINER_PREFIX)),
                "{args:?}"
            );
        }

        let listing = list_managed();
        assert!(
            listing.iter().any(|arg| arg.contains(MANAGED_LABEL)),
            "{listing:?}"
        );
        assert!(
            listing.iter().any(|arg| arg == "--all"),
            "cleanup must see stopped containers too: {listing:?}"
        );
    }

    /// Removing an application takes its anonymous volumes with it. "Deleted"
    /// meaning "mostly deleted" would be a retention promise Ephemeral does not
    /// keep.
    #[test]
    fn removal_takes_the_volumes_too() {
        let args = remove(&app());
        assert_flag(&args, "--volumes");
        assert_flag(&args, "--force");
    }

    /// Output is requested as JSON rather than parsed out of human formatting,
    /// which is the thing that breaks between Docker versions.
    #[test]
    fn machine_readable_output_is_requested_explicitly() {
        assert!(list_managed().iter().any(|arg| arg.contains("json")));
        assert!(version().iter().any(|arg| arg.contains("{{")));
    }
}
