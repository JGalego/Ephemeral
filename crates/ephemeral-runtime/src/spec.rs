//! What a runtime is asked to run, and under what confinement.
//!
//! A [`ContainerSpec`] is built from an application's manifest and the
//! permissions its owner actually granted — never from the manifest alone. The
//! manifest says what an application *wants*; the ledger says what it *has*.
//! Building the spec from the first would let an application confine itself.
//!
//! Everything here is data. Turning it into something a runtime executes is
//! [`crate::docker::command`], which is a pure function so the confinement can
//! be asserted in tests without a container runtime present.

use std::path::{Path, PathBuf};

use ephemeral_core::{
    AppId,
    manifest::ResourceLimits,
    permission::{AppPermission, HostScope, PathScope},
};

use crate::RuntimeError;

/// Where the application's own storage appears inside the sandbox.
pub const DATA_MOUNT: &str = "/data";

/// Where granted host directories appear inside the sandbox.
const GRANT_MOUNT_ROOT: &str = "/mnt";

/// The identity a container runs as when nothing better is known.
///
/// `nobody`. Not root, and not configurable to root: the runtime may substitute
/// the invoking user's identity so that writable mounts work, but it has no path
/// that produces uid 0.
pub const UNPRIVILEGED_USER: &str = "65534:65534";

/// The directories on this machine that scopes are resolved against.
///
/// A [`PathScope`] is written the way a person writes it — `~/Downloads/report`
/// — which is not a path any operating system will accept. Resolving it needs
/// the home directory, so it is passed in rather than read from the environment:
/// the function that decides what gets mounted stays pure and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    /// The user's home directory, which `~` means.
    pub home: PathBuf,

    /// Where this application's own data lives on the host.
    pub data_dir: PathBuf,
}

impl HostPaths {
    /// Resolves a scope to a real directory on this machine.
    ///
    /// Returns `None` for anything that does not resolve to an absolute path,
    /// which is the safe direction: an unresolvable scope becomes no mount
    /// rather than a mount of something unintended.
    #[must_use]
    pub fn resolve(&self, scope: &PathScope) -> Option<PathBuf> {
        let written = scope.display_path();

        // Joined segment by segment rather than in one piece. A scope is always
        // written with forward slashes, and joining the whole tail at once
        // would leave them embedded in the middle of an otherwise native path —
        // `C:\Users\ana\Downloads/apartments`. That is what the user is shown
        // and what the runtime is handed, so it should look like a path from
        // this machine rather than from two.
        let resolved = if written == "~" {
            self.home.clone()
        } else if let Some(rest) = written.strip_prefix("~/") {
            rest.split('/')
                .fold(self.home.clone(), |path, segment| path.join(segment))
        } else {
            PathBuf::from(&written)
        };

        // Belt and braces. `PathScope::parse` already refuses `..`, but this is
        // the last point before a path is handed to a container runtime, and a
        // relative or traversing path here would escape the intended region.
        if resolved
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return None;
        }
        if !is_absolute(&resolved) {
            return None;
        }

        Some(resolved)
    }
}

/// Whether a path is anchored, by POSIX rules or by Windows drive rules.
///
/// `Path::is_absolute` answers for the *host*, which is the wrong question: the
/// answer must not depend on which machine is deciding what a manifest means.
fn is_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.starts_with('/') || {
        let bytes = text.as_bytes();
        bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'/' || bytes[2] == b'\\')
    }
}

/// A directory from the host made visible inside the sandbox.
///
/// Mounts are the sharpest edge in the whole product: they are the only reason a
/// generated application can see anything of yours at all. Each one exists
/// because a specific permission was granted over a specific path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The real directory on this machine.
    pub host_path: PathBuf,

    /// Where it appears inside the sandbox.
    pub container_path: String,

    /// Whether the application may change what is there.
    pub writable: bool,
}

impl Mount {
    /// A mount the application can read but not change.
    #[must_use]
    pub fn read_only(host_path: impl Into<PathBuf>, container_path: impl Into<String>) -> Self {
        Self {
            host_path: host_path.into(),
            container_path: container_path.into(),
            writable: false,
        }
    }

    /// A mount the application can write to.
    #[must_use]
    pub fn writable(host_path: impl Into<PathBuf>, container_path: impl Into<String>) -> Self {
        Self {
            host_path: host_path.into(),
            container_path: container_path.into(),
            writable: true,
        }
    }
}

/// Access that was granted but will not be given, and why.
///
/// Recorded rather than dropped. Silently ignoring a grant would make the
/// sandbox disagree with what the user was told they allowed, and a user is
/// entitled to know that a decision they made is not being honoured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedAccess {
    /// What was granted, in the user's language.
    pub granted: String,

    /// Why it will not be given effect.
    pub reason: String,
}

/// What the application may reach over the network.
///
/// Denied by default. There is deliberately no "allow everything" variant that
/// can be reached by omission — reaching the internet takes an explicit
/// [`Egress::Anywhere`], which exists so the *user* can grant it knowingly, and
/// which the interface flags as high risk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Egress {
    /// No network at all.
    #[default]
    Denied,

    /// Only these destinations.
    ///
    /// Enforcing this properly needs a filtering layer a container runtime does
    /// not provide by itself. Until that exists, a runtime asked for this must
    /// refuse to start rather than quietly grant the whole internet — see
    /// [`crate::RuntimeError::CannotEnforce`].
    AllowList(Vec<HostScope>),

    /// Anywhere. Granted explicitly, never by default.
    Anywhere,
}

impl Egress {
    /// How this policy is described to a person.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Denied => "no network access at all".to_owned(),
            Self::AllowList(hosts) => {
                let names: Vec<String> = hosts.iter().map(HostScope::as_written).collect();
                format!("network access to {}", names.join(", "))
            }
            Self::Anywhere => "unrestricted network access".to_owned(),
        }
    }

    /// Whether this policy lets the application reach anything at all.
    ///
    /// An allow-list counts. A runtime that cannot filter by destination
    /// cannot honour one, and the honest answer to "may it reach the network"
    /// is yes — the question of *how much* is what it cannot enforce.
    #[must_use]
    pub fn is_permitted(&self) -> bool {
        !matches!(self, Self::Denied)
    }
}

/// A port the application listens on, and where it is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortBinding {
    /// The port inside the sandbox.
    pub container_port: u16,

    /// The port on the host.
    pub host_port: u16,

    /// Whether anything other than this machine may connect.
    ///
    /// False binds to loopback. Publishing beyond loopback is a separate
    /// decision a person makes, never a consequence of an application asking to
    /// listen.
    pub publicly_reachable: bool,
}

impl PortBinding {
    /// A port reachable only from this machine.
    #[must_use]
    pub fn loopback(container_port: u16, host_port: u16) -> Self {
        Self {
            container_port,
            host_port,
            publicly_reachable: false,
        }
    }

    /// The host address this binds to.
    #[must_use]
    pub fn host_address(&self) -> &'static str {
        if self.publicly_reachable {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }
}

/// Everything a runtime needs in order to run one application, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    /// Which application this is for.
    pub app: AppId,

    /// The image to run. Pinned by digest wherever possible.
    pub image: String,

    /// The command, already split into arguments. Never shell-parsed.
    pub entrypoint: Vec<String>,

    /// What it may consume.
    pub limits: ResourceLimits,

    /// What of the host it can see.
    pub mounts: Vec<Mount>,

    /// What it may reach.
    pub egress: Egress,

    /// Where it listens.
    pub ports: Vec<PortBinding>,

    /// The **names** of the settings whose values the runtime will inject.
    ///
    /// Names, never values. A value would otherwise travel through an argument
    /// vector — visible in the process table, in an error message, and in the
    /// audit log. Values are supplied separately at the moment of starting, in
    /// [`crate::Secrets`], and are passed through the child process's own
    /// environment.
    pub environment_names: Vec<String>,

    /// The directory the application starts in, inside the sandbox.
    pub working_dir: String,

    /// The identity the application runs as, as `uid:gid`.
    ///
    /// Never root. The runtime may replace this with the invoking user's own
    /// identity so that writable mounts behave, but there is no path here that
    /// produces uid 0.
    pub user: String,

    /// Access the user granted that this specification will not give effect to.
    pub refused: Vec<RefusedAccess>,
}

impl ContainerSpec {
    /// A specification that can do nothing but run its entrypoint.
    ///
    /// No mounts, no network, no ports, no environment. Everything an
    /// application can reach is added deliberately from a granted permission, so
    /// forgetting to add something yields less access rather than more.
    #[must_use]
    pub fn minimal(app: AppId, image: impl Into<String>, entrypoint: Vec<String>) -> Self {
        Self {
            app,
            image: image.into(),
            entrypoint,
            limits: ResourceLimits::default(),
            mounts: Vec::new(),
            egress: Egress::Denied,
            ports: Vec::new(),
            environment_names: Vec::new(),
            working_dir: "/app".to_owned(),
            user: UNPRIVILEGED_USER.to_owned(),
            refused: Vec::new(),
        }
    }

    /// The specification for running `app` under the permissions it was granted.
    ///
    /// The direction matters: this takes granted capabilities, not the ones the
    /// manifest requests, so an application cannot widen its own sandbox by
    /// asking for more.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CannotEnforce`] if the application's declared limits are
    /// not enforceable values.
    pub fn from_grants(
        app: AppId,
        image: impl Into<String>,
        entrypoint: Vec<String>,
        limits: ResourceLimits,
        granted: &[AppPermission],
        paths: &HostPaths,
    ) -> Result<Self, RuntimeError> {
        if !limits.is_valid() {
            return Err(RuntimeError::CannotEnforce {
                control: "this application's resource limits".to_owned(),
                reason: format!(
                    "the manifest declares a limit of zero, which is not a ceiling: {limits}"
                ),
            });
        }

        let plan = access_from_grants(granted, paths);
        let egress = plan.egress();

        let mut mounts = vec![Mount::writable(paths.data_dir.clone(), DATA_MOUNT)];
        mounts.extend(plan.mounts);

        Ok(Self {
            limits,
            mounts,
            egress,
            ports: plan.ports,
            environment_names: plan.environment_names,
            refused: plan.refused,
            ..Self::minimal(app, image, entrypoint)
        })
    }

    /// The container name Ephemeral gives this application.
    ///
    /// Prefixed so that cleanup can find Ephemeral's containers without touching
    /// anything else the user is running — reaping by a bare application id
    /// would be a fine way to destroy somebody's unrelated work.
    #[must_use]
    pub fn container_name(&self) -> String {
        format!("{}{}", crate::CONTAINER_PREFIX, self.app)
    }

    /// Whether this specification grants any access to the host at all.
    ///
    /// The application's own storage does not count: every application has that,
    /// and it holds nothing but what the application itself put there.
    ///
    /// Used by the interface to say "this app can see nothing of yours" when
    /// that is true, which is the common case and worth saying.
    #[must_use]
    pub fn is_isolated(&self) -> bool {
        self.host_mounts().next().is_none()
            && self.egress == Egress::Denied
            && self.ports.is_empty()
    }

    /// The mounts that expose something of the user's, as opposed to the
    /// application's own storage.
    pub fn host_mounts(&self) -> impl Iterator<Item = &Mount> {
        self.mounts
            .iter()
            .filter(|mount| mount.container_path != DATA_MOUNT)
    }

    /// Whether the application is reachable from outside this machine.
    #[must_use]
    pub fn is_publicly_reachable(&self) -> bool {
        self.ports.iter().any(|port| port.publicly_reachable)
    }
}

/// Turns the permissions an application was *granted* into the access a sandbox
/// will actually give it.
///
/// Anything not recognised here yields no access, which is the safe direction
/// for a function that will gain cases over time.
#[must_use]
pub fn access_from_grants(granted: &[AppPermission], paths: &HostPaths) -> AccessPlan {
    let mut plan = AccessPlan::default();

    for permission in granted {
        match permission {
            AppPermission::FilesystemRead { scope } => plan.add_mount(scope, false, paths),
            AppPermission::FilesystemWrite { scope } => plan.add_mount(scope, true, paths),
            AppPermission::NetworkOutbound { scope } => plan.egress_scopes.push(scope.clone()),
            AppPermission::NetworkInbound { port } => {
                plan.ports.push(PortBinding::loopback(*port, *port));
            }
            AppPermission::ReadEnvironment { name } => plan.environment_names.push(name.clone()),
            // Devices and process execution are not expressed as mounts or
            // ports. Granting one must not silently widen the filesystem or the
            // network; where they are enforced is the sandbox's own
            // configuration, not this plan.
            AppPermission::ExecuteProcesses
            | AppPermission::Camera
            | AppPermission::Microphone
            | AppPermission::Location => {}
            other => plan.refused.push(RefusedAccess {
                granted: other.capability().to_owned(),
                reason: "this version of Ephemeral does not know how to confine it".to_owned(),
            }),
        }
    }

    plan
}

/// The access a set of grants adds up to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessPlan {
    /// Host directories to expose.
    pub mounts: Vec<Mount>,

    /// Destinations the application may reach.
    pub egress_scopes: Vec<HostScope>,

    /// Ports it listens on.
    pub ports: Vec<PortBinding>,

    /// Names of settings whose values the runtime will inject.
    pub environment_names: Vec<String>,

    /// Grants that will not be given effect, and why.
    pub refused: Vec<RefusedAccess>,
}

impl AccessPlan {
    /// The egress policy these grants add up to.
    #[must_use]
    pub fn egress(&self) -> Egress {
        if self.egress_scopes.is_empty() {
            Egress::Denied
        } else if self.egress_scopes.iter().any(HostScope::is_anywhere) {
            Egress::Anywhere
        } else {
            Egress::AllowList(self.egress_scopes.clone())
        }
    }

    /// Records a filesystem grant as a mount, or as a refusal.
    fn add_mount(&mut self, scope: &PathScope, writable: bool, paths: &HostPaths) {
        // SECURITY.md promises never to mount an entire root into a generated
        // container. The permission model flags such a scope; this refuses it,
        // so the promise does not depend on a prompt being read.
        if scope.is_whole_root() {
            self.refused.push(RefusedAccess {
                granted: scope.as_written(),
                reason: "Ephemeral does not mount an entire drive or home directory into a \
                         generated application, whatever was granted"
                    .to_owned(),
            });
            return;
        }

        let Some(host_path) = paths.resolve(scope) else {
            self.refused.push(RefusedAccess {
                granted: scope.as_written(),
                reason: "this does not resolve to a real directory on this machine".to_owned(),
            });
            return;
        };

        let container_path = format!("{GRANT_MOUNT_ROOT}/{}", mount_label(&scope.display_path()));

        // A read grant and a write grant over the same region must not produce
        // two mounts at one point; the writable one wins, because the user
        // granted it.
        if let Some(existing) = self
            .mounts
            .iter_mut()
            .find(|mount| mount.container_path == container_path)
        {
            existing.writable |= writable;
            return;
        }

        self.mounts.push(Mount {
            host_path,
            container_path,
            writable,
        });
    }
}

/// A filesystem-safe label for a mount point, derived from the host path.
fn mount_label(path: &str) -> String {
    let label: String = path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label = label.trim_matches('-').to_ascii_lowercase();

    // Collapse runs so a deep path does not become a wall of hyphens.
    let mut out = String::with_capacity(label.len());
    let mut last_hyphen = false;
    for c in label.chars() {
        if c == '-' {
            if !last_hyphen {
                out.push(c);
            }
            last_hyphen = true;
        } else {
            out.push(c);
            last_hyphen = false;
        }
    }

    if out.is_empty() {
        "data".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppId {
        AppId::parse("csv-comparator").unwrap()
    }

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    fn host(name: &str) -> HostScope {
        HostScope::parse(name).unwrap()
    }

    fn paths() -> HostPaths {
        HostPaths {
            home: PathBuf::from("/home/ana"),
            data_dir: PathBuf::from("/home/ana/.local/share/ephemeral/apps/csv-comparator/data"),
        }
    }

    /// The default is nothing. Forgetting to add access must yield less, never
    /// more.
    #[test]
    fn a_minimal_specification_can_reach_nothing() {
        let spec = ContainerSpec::minimal(app(), "alpine", vec!["true".to_owned()]);

        assert!(spec.mounts.is_empty());
        assert_eq!(spec.egress, Egress::Denied);
        assert!(spec.ports.is_empty());
        assert!(spec.environment_names.is_empty());
        assert!(spec.is_isolated());
        assert_ne!(spec.user, "0:0", "nothing here may run as root");
    }

    #[test]
    fn egress_is_denied_by_default_and_by_omission() {
        assert_eq!(Egress::default(), Egress::Denied);
        assert_eq!(AccessPlan::default().egress(), Egress::Denied);
    }

    /// The plan is built from what was *granted*. An application cannot widen
    /// its own sandbox by asking for more.
    #[test]
    fn access_comes_from_grants() {
        let plan = access_from_grants(
            &[
                AppPermission::read(scope("~/Downloads/apartments/**")),
                AppPermission::outbound(host("api.example.com")),
                AppPermission::NetworkInbound { port: 8080 },
                AppPermission::ReadEnvironment {
                    name: "API_KEY".to_owned(),
                },
            ],
            &paths(),
        );

        assert_eq!(plan.mounts.len(), 1);
        assert!(
            !plan.mounts[0].writable,
            "a read grant must not mount writable"
        );
        assert_eq!(plan.ports, vec![PortBinding::loopback(8080, 8080)]);
        assert_eq!(plan.environment_names, vec!["API_KEY".to_owned()]);
        assert_eq!(
            plan.egress(),
            Egress::AllowList(vec![host("api.example.com")])
        );
        assert!(plan.refused.is_empty());
    }

    /// An application gets exactly the access its owner granted — a spec built
    /// from an empty ledger can reach nothing of the user's, however much the
    /// manifest asked for.
    #[test]
    fn a_spec_built_from_no_grants_is_isolated() {
        let spec = ContainerSpec::from_grants(
            app(),
            "python:3.12-slim",
            vec!["python".to_owned(), "main.py".to_owned()],
            ResourceLimits::default(),
            &[],
            &paths(),
        )
        .unwrap();

        assert!(spec.is_isolated());
        assert_eq!(spec.host_mounts().count(), 0);
        assert_eq!(
            spec.mounts.len(),
            1,
            "an app still gets its own storage, which holds only what it wrote"
        );
        assert_eq!(spec.mounts[0].container_path, DATA_MOUNT);
        assert!(spec.mounts[0].writable);
    }

    /// `~` is how a person writes a path, not something a container runtime can
    /// mount. Handing it over unresolved would create a directory called `~`.
    #[test]
    fn home_relative_scopes_resolve_to_real_directories() {
        let plan = access_from_grants(&[AppPermission::read(scope("~/Downloads/**"))], &paths());

        assert_eq!(
            plan.mounts[0].host_path,
            PathBuf::from("/home/ana/Downloads")
        );
        assert!(
            !plan.mounts[0].host_path.to_string_lossy().contains('~'),
            "an unresolved ~ would be mounted as a literal directory name"
        );
    }

    /// A resolved path is handed to a container runtime and shown to the user,
    /// so it should read like a path from this machine rather than half a path
    /// from each. Only Windows can fail this — a scope is written with forward
    /// slashes, and gluing its tail onto a native prefix in one piece leaves
    /// them embedded in the middle.
    #[test]
    fn a_resolved_path_does_not_mix_separators() {
        let paths = paths();
        let resolved = paths.resolve(&scope("~/Downloads/apartments/**")).unwrap();

        assert_eq!(resolved, paths.home.join("Downloads").join("apartments"));

        let rendered = resolved.to_string_lossy().into_owned();
        let tail = &rendered[paths.home.to_string_lossy().len()..];
        let foreign = if std::path::MAIN_SEPARATOR == '/' {
            '\\'
        } else {
            '/'
        };

        assert!(!tail.contains(foreign), "{rendered} mixes path separators");
    }

    #[test]
    fn a_write_grant_mounts_writable_and_a_read_grant_does_not() {
        let read = access_from_grants(&[AppPermission::read(scope("~/a/**"))], &paths());
        assert!(!read.mounts[0].writable);

        let write = access_from_grants(&[AppPermission::write(scope("~/a/**"))], &paths());
        assert!(write.mounts[0].writable);
    }

    /// Read and write over the same region are one mount, not two at the same
    /// point — and the one the user granted wins.
    #[test]
    fn overlapping_grants_produce_one_mount() {
        let plan = access_from_grants(
            &[
                AppPermission::read(scope("~/a/**")),
                AppPermission::write(scope("~/a/**")),
            ],
            &paths(),
        );

        assert_eq!(plan.mounts.len(), 1);
        assert!(plan.mounts[0].writable);
    }

    /// SECURITY.md promises this, so it has to hold even when a grant says
    /// otherwise. A refusal is recorded rather than silently dropped.
    #[test]
    fn an_entire_root_is_never_mounted_however_it_was_granted() {
        for whole_root in ["~/**", "/**", "C:/**"] {
            let plan = access_from_grants(&[AppPermission::read(scope(whole_root))], &paths());

            assert!(plan.mounts.is_empty(), "{whole_root} must not be mounted");
            assert_eq!(plan.refused.len(), 1, "{whole_root} must be reported");
            assert!(plan.refused[0].reason.contains("does not mount"));
        }
    }

    /// Granting a device or process capability must not quietly widen the
    /// filesystem or the network.
    #[test]
    fn device_and_process_grants_add_no_host_access() {
        let plan = access_from_grants(
            &[
                AppPermission::Camera,
                AppPermission::Microphone,
                AppPermission::Location,
                AppPermission::ExecuteProcesses,
            ],
            &paths(),
        );

        assert!(plan.mounts.is_empty());
        assert!(plan.ports.is_empty());
        assert_eq!(plan.egress(), Egress::Denied);
    }

    /// Unrestricted egress must be reachable only by an explicit grant of it,
    /// and must not be produced by a list of ordinary hosts.
    #[test]
    fn anywhere_egress_requires_granting_anywhere() {
        let named = access_from_grants(
            &[AppPermission::outbound(host("api.example.com"))],
            &paths(),
        );
        assert_ne!(named.egress(), Egress::Anywhere);

        let anywhere = access_from_grants(
            &[
                AppPermission::outbound(host("api.example.com")),
                AppPermission::outbound(host("*")),
            ],
            &paths(),
        );
        assert_eq!(anywhere.egress(), Egress::Anywhere);
    }

    /// Listening on a port must not, by itself, expose anything beyond this
    /// machine.
    #[test]
    fn a_listening_port_is_bound_to_loopback() {
        let plan = access_from_grants(&[AppPermission::NetworkInbound { port: 8080 }], &paths());

        assert!(!plan.ports[0].publicly_reachable);
        assert_eq!(plan.ports[0].host_address(), "127.0.0.1");

        let spec = ContainerSpec::from_grants(
            app(),
            "alpine",
            vec![],
            ResourceLimits::default(),
            &[AppPermission::NetworkInbound { port: 8080 }],
            &paths(),
        )
        .unwrap();
        assert!(!spec.is_publicly_reachable());
    }

    /// A secret's value must not be able to enter the specification at all —
    /// the type carries names, and there is nowhere to put a value.
    #[test]
    fn a_specification_carries_secret_names_and_not_values() {
        let spec = ContainerSpec::from_grants(
            app(),
            "alpine",
            vec![],
            ResourceLimits::default(),
            &[AppPermission::ReadEnvironment {
                name: "API_KEY".to_owned(),
            }],
            &paths(),
        )
        .unwrap();

        assert_eq!(spec.environment_names, vec!["API_KEY".to_owned()]);
        assert!(!format!("{spec:?}").contains("sk-"));
    }

    /// A limit of zero is not a ceiling. Refusing to build the spec is better
    /// than starting an application with a control that does nothing.
    #[test]
    fn unusable_limits_are_refused_rather_than_applied() {
        let broken = ResourceLimits {
            memory_mib: 0,
            ..ResourceLimits::default()
        };

        let error =
            ContainerSpec::from_grants(app(), "alpine", vec![], broken, &[], &paths()).unwrap_err();

        assert!(matches!(error, RuntimeError::CannotEnforce { .. }));
    }

    #[test]
    fn container_names_are_namespaced_to_ephemeral() {
        let spec = ContainerSpec::minimal(app(), "alpine", vec![]);
        assert!(spec.container_name().starts_with(crate::CONTAINER_PREFIX));
        assert!(spec.container_name().ends_with("csv-comparator"));
    }

    #[test]
    fn mount_labels_are_readable_and_safe() {
        assert_eq!(
            mount_label("~/Downloads/apartments"),
            "downloads-apartments"
        );
        assert_eq!(mount_label("/etc/hosts"), "etc-hosts");
        assert_eq!(mount_label("~"), "data");
    }

    /// What a path means must not depend on which machine is asking. A Windows
    /// host deciding that `/home/ana/x` is relative would silently turn a mount
    /// into a refusal, or worse.
    #[test]
    fn paths_are_judged_by_the_manifest_rules_not_the_host() {
        assert!(is_absolute(Path::new("/home/ana")));
        assert!(is_absolute(Path::new("C:/Users/ana")));
        assert!(is_absolute(Path::new(r"C:\Users\ana")));
        assert!(!is_absolute(Path::new("Downloads")));
        assert!(!is_absolute(Path::new("")));
    }

    #[test]
    fn egress_describes_itself_for_the_interface() {
        assert!(Egress::Denied.describe().contains("no network"));
        assert!(Egress::Anywhere.describe().contains("unrestricted"));
        assert!(
            Egress::AllowList(vec![host("api.example.com")])
                .describe()
                .contains("api.example.com")
        );
    }
}
