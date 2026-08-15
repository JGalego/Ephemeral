//! Application permissions: what **one generated app** is allowed to do.
//!
//! Two representations of the same thing, kept deliberately in sync:
//!
//! - [`AppPermissions`] is the declarative form that lives in an app's manifest.
//!   It is what a person reads and edits.
//! - [`AppPermission`] is the flat capability form the ledger stores, grants and
//!   checks. It is what a machine decides with.
//!
//! [`AppPermissions::capabilities`] converts the first into the second, so there
//! is one source of truth and no chance of the readable version promising
//! something the enforced version does not.
//!
//! ## The invariant
//!
//! An application starts with **nothing**. [`AppPermissions::default`] denies
//! everything, and every capability an app holds was written into its manifest
//! and approved by a person. Nothing here reads Ephemeral's own permissions,
//! because an app inherits none of them ([ADR-0003]).
//!
//! [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{HostScope, MetaPermission, PathScope, RiskLevel};

/// Something one generated application may be allowed to do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AppPermission {
    /// Read files in a region of the filesystem.
    FilesystemRead {
        /// Where.
        scope: PathScope,
    },

    /// Create, modify or delete files in a region of the filesystem.
    FilesystemWrite {
        /// Where.
        scope: PathScope,
    },

    /// Make outbound network connections to a destination.
    NetworkOutbound {
        /// Where to.
        scope: HostScope,
    },

    /// Accept inbound connections on a port.
    ///
    /// Ports are bound to loopback unless the user decides otherwise; the
    /// runtime, not the application, decides where a port is published.
    NetworkInbound {
        /// Which port inside the application.
        port: u16,
    },

    /// Run other programs.
    ///
    /// Rarely justified for a generated app, and correspondingly high-risk: it
    /// is the capability that most weakens the value of every other limit.
    ExecuteProcesses,

    /// Read one environment variable by name.
    ///
    /// The *value* is injected by the runtime from secure storage and never
    /// appears in the manifest, the interface or any log.
    ReadEnvironment {
        /// Which variable.
        name: String,
    },

    /// Use the camera.
    Camera,

    /// Use the microphone.
    Microphone,

    /// Read the device's location.
    Location,
}

impl AppPermission {
    /// Convenience constructor for a read scope.
    #[must_use]
    pub fn read(scope: PathScope) -> Self {
        Self::FilesystemRead { scope }
    }

    /// Convenience constructor for a write scope.
    #[must_use]
    pub fn write(scope: PathScope) -> Self {
        Self::FilesystemWrite { scope }
    }

    /// Convenience constructor for an egress scope.
    #[must_use]
    pub fn outbound(scope: HostScope) -> Self {
        Self::NetworkOutbound { scope }
    }

    /// The stable capability name, matching the serialised form.
    #[must_use]
    pub fn capability(&self) -> &'static str {
        match self {
            Self::FilesystemRead { .. } => "filesystem_read",
            Self::FilesystemWrite { .. } => "filesystem_write",
            Self::NetworkOutbound { .. } => "network_outbound",
            Self::NetworkInbound { .. } => "network_inbound",
            Self::ExecuteProcesses => "execute_processes",
            Self::ReadEnvironment { .. } => "read_environment",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::Location => "location",
        }
    }

    /// Whether a grant of `self` covers a request for `requested`.
    ///
    /// Scoped capabilities compare scopes; the rest compare by identity. A
    /// capability never satisfies a different capability.
    #[must_use]
    pub fn satisfies(&self, requested: &Self) -> bool {
        match (self, requested) {
            (Self::FilesystemRead { scope: held }, Self::FilesystemRead { scope: want })
            | (Self::FilesystemWrite { scope: held }, Self::FilesystemWrite { scope: want }) => {
                held.contains(want)
            }
            (Self::NetworkOutbound { scope: held }, Self::NetworkOutbound { scope: want }) => {
                held.contains(want)
            }
            (Self::NetworkInbound { port: held }, Self::NetworkInbound { port: want }) => {
                held == want
            }
            (Self::ReadEnvironment { name: held }, Self::ReadEnvironment { name: want }) => {
                held == want
            }
            // The remaining capabilities carry no scope, so identity is the whole
            // question. Capability names are unique per variant, which is what
            // makes this comparison safe rather than convenient.
            (held, want) => held.capability() == want.capability(),
        }
    }

    /// The meta-permission Ephemeral must itself hold for this app capability to
    /// be usable.
    ///
    /// This is the *ceiling* rule from [ADR-0003]: an application's grant is
    /// necessary but not sufficient. Revoking Ephemeral's own camera access
    /// disables the camera for every app, whatever their manifests say.
    ///
    /// [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md
    #[must_use]
    pub fn required_meta(&self) -> MetaPermission {
        match self {
            Self::FilesystemRead { scope } => MetaPermission::FilesystemRead {
                scope: scope.clone(),
            },
            Self::FilesystemWrite { scope } => MetaPermission::FilesystemWrite {
                scope: scope.clone(),
            },
            // Any networking an app does is Ephemeral's runtime reaching the
            // network on its behalf.
            Self::NetworkOutbound { .. } | Self::NetworkInbound { .. } => {
                MetaPermission::NetworkAccess
            }
            Self::ExecuteProcesses => MetaPermission::ExecuteProcesses,
            Self::ReadEnvironment { .. } => MetaPermission::ReadEnvironment,
            Self::Camera => MetaPermission::Camera,
            Self::Microphone => MetaPermission::Microphone,
            Self::Location => MetaPermission::Location,
        }
    }

    /// How dangerous this capability is for a generated application.
    ///
    /// Note that the same capability is riskier for an app than for Ephemeral:
    /// the code holding it was written by a model minutes ago and reviewed by
    /// nobody.
    #[must_use]
    pub fn risk(&self) -> RiskLevel {
        match self {
            Self::ExecuteProcesses => RiskLevel::Critical,
            Self::FilesystemWrite { scope } => {
                if scope.is_whole_root() {
                    RiskLevel::Critical
                } else {
                    RiskLevel::High
                }
            }
            Self::FilesystemRead { scope } => {
                if scope.is_whole_root() {
                    RiskLevel::Critical
                } else if scope.is_recursive() {
                    RiskLevel::Medium
                } else {
                    RiskLevel::Low
                }
            }
            Self::NetworkOutbound { scope } => {
                if scope.is_anywhere() {
                    RiskLevel::High
                } else {
                    RiskLevel::Medium
                }
            }
            Self::Camera | Self::Microphone | Self::Location => RiskLevel::High,
            Self::ReadEnvironment { .. } => RiskLevel::Medium,
            Self::NetworkInbound { .. } => RiskLevel::Low,
        }
    }

    /// What this permission lets the application do, in plain language.
    ///
    /// Phrased to complete the sentence "`<app name>` wants to …".
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::FilesystemRead { scope } => {
                if scope.is_recursive() {
                    format!("read the files in {}", scope.display_path())
                } else {
                    format!("read {}", scope.display_path())
                }
            }
            Self::FilesystemWrite { scope } => {
                if scope.is_recursive() {
                    format!("save files in {}", scope.display_path())
                } else {
                    format!("change {}", scope.display_path())
                }
            }
            Self::NetworkOutbound { scope } => {
                if scope.is_anywhere() {
                    "connect to anywhere on the internet".to_owned()
                } else {
                    format!("connect to {scope}")
                }
            }
            Self::NetworkInbound { port } => {
                format!("accept connections on port {port}")
            }
            Self::ExecuteProcesses => "run other programs".to_owned(),
            Self::ReadEnvironment { name } => format!("use the setting called {name}"),
            Self::Camera => "use the camera".to_owned(),
            Self::Microphone => "use the microphone".to_owned(),
            Self::Location => "know where this device is".to_owned(),
        }
    }

    /// What allowing this permission means, and what remains denied.
    #[must_use]
    pub fn consequences(&self) -> String {
        match self {
            Self::FilesystemRead { scope } => format!(
                "It can read what is at {}. It cannot change those files, and it cannot \
                 see anything else on this device.",
                scope.display_path()
            ),
            Self::FilesystemWrite { scope } => format!(
                "It can create and change files at {}, and nowhere else.",
                scope.display_path()
            ),
            Self::NetworkOutbound { scope } if scope.is_anywhere() => {
                "It can send data anywhere on the internet, including data it read from \
                 your files. This is the permission most worth thinking twice about."
                    .to_owned()
            }
            Self::NetworkOutbound { scope } => format!(
                "It can exchange data with {scope}, and no other destination. It cannot \
                 reach the rest of the internet."
            ),
            Self::NetworkInbound { port } => format!(
                "You can open the app on port {port}. It is reachable only from this \
                 device unless you say otherwise."
            ),
            Self::ExecuteProcesses => "It can start other programs inside its sandbox. \
                 This weakens the other limits on it, so grant it only if the app \
                 genuinely needs it."
                .to_owned(),
            Self::ReadEnvironment { name } => format!(
                "It can use the value of {name}. The value is given to the app when it \
                 runs; it is never written into the app or shown in logs."
            ),
            Self::Camera => "It can take pictures and video while it is running.".to_owned(),
            Self::Microphone => "It can record audio while it is running.".to_owned(),
            Self::Location => "It can read this device's location while it is running.".to_owned(),
        }
    }
}

impl fmt::Display for AppPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FilesystemRead { scope } | Self::FilesystemWrite { scope } => {
                write!(f, "{}({scope})", self.capability())
            }
            Self::NetworkOutbound { scope } => write!(f, "network_outbound({scope})"),
            Self::NetworkInbound { port } => write!(f, "network_inbound({port})"),
            Self::ReadEnvironment { name } => write!(f, "read_environment({name})"),
            other => f.write_str(other.capability()),
        }
    }
}

/// One filesystem rule in an application's manifest.
///
/// Serialises as a single-key mapping, which is what makes a manifest readable:
///
/// ```yaml
/// filesystem:
///   - read: ~/Downloads/apartments/**
///   - write: ~/Documents/reports/**
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FilesystemRuleFields", into = "FilesystemRuleFields")]
pub enum FilesystemRule {
    /// May read this region.
    Read(PathScope),
    /// May write this region.
    Write(PathScope),
    /// May read and write this region.
    ReadWrite(PathScope),
}

/// The on-disk shape of a [`FilesystemRule`]: a mapping with exactly one key.
///
/// Serde's own enum representations would write a YAML tag (`!read ~/x`) or a
/// nested map, neither of which reads like something a person would write. A
/// manifest is a document users are expected to check, so the format is chosen
/// for them rather than for the serialiser.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesystemRuleFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    read: Option<PathScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write: Option<PathScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    read_write: Option<PathScope>,
}

impl From<FilesystemRule> for FilesystemRuleFields {
    fn from(rule: FilesystemRule) -> Self {
        let (read, write, read_write) = match rule {
            FilesystemRule::Read(scope) => (Some(scope), None, None),
            FilesystemRule::Write(scope) => (None, Some(scope), None),
            FilesystemRule::ReadWrite(scope) => (None, None, Some(scope)),
        };
        Self {
            read,
            write,
            read_write,
        }
    }
}

impl TryFrom<FilesystemRuleFields> for FilesystemRule {
    type Error = &'static str;

    fn try_from(fields: FilesystemRuleFields) -> Result<Self, Self::Error> {
        // Exactly one mode per rule. A rule with two modes, or none, is
        // ambiguous about what was approved, and an ambiguous permission is
        // refused rather than interpreted.
        match (fields.read, fields.write, fields.read_write) {
            (Some(scope), None, None) => Ok(Self::Read(scope)),
            (None, Some(scope), None) => Ok(Self::Write(scope)),
            (None, None, Some(scope)) => Ok(Self::ReadWrite(scope)),
            (None, None, None) => {
                Err("a filesystem rule needs one of 'read', 'write' or 'read_write'")
            }
            _ => Err("a filesystem rule must name exactly one of 'read', 'write' or 'read_write'"),
        }
    }
}

impl FilesystemRule {
    /// The capabilities this rule grants.
    #[must_use]
    pub fn capabilities(&self) -> Vec<AppPermission> {
        match self {
            Self::Read(scope) => vec![AppPermission::read(scope.clone())],
            Self::Write(scope) => vec![AppPermission::write(scope.clone())],
            Self::ReadWrite(scope) => vec![
                AppPermission::read(scope.clone()),
                AppPermission::write(scope.clone()),
            ],
        }
    }
}

/// An application's network permissions.
///
/// Denies everything by default. `outbound` must be explicitly enabled *and*
/// destinations listed: enabling outbound with an empty allow-list grants
/// nothing, because "the whole internet" has to be written down as `*` before
/// anyone can approve it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkPolicy {
    /// Whether the application may make outbound connections at all.
    pub outbound: bool,

    /// Where it may connect to. Empty means nowhere.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<HostScope>,

    /// Ports the application listens on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inbound_ports: Vec<u16>,
}

/// An application's process permissions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessPolicy {
    /// Whether the application may start other programs.
    pub execute: bool,
}

/// An application's device permissions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DevicePolicy {
    /// Whether the application may use the camera.
    pub camera: bool,
    /// Whether the application may use the microphone.
    pub microphone: bool,
    /// Whether the application may read the device's location.
    pub location: bool,
}

/// Everything one generated application is permitted to do.
///
/// This is the `permissions:` block of an app manifest:
///
/// ```yaml
/// permissions:
///   filesystem:
///     - read: ~/Downloads/apartments/**
///   network:
///     outbound: false
///   process:
///     execute: false
///   devices:
///     camera: false
///     microphone: false
///     location: false
/// ```
///
/// [`AppPermissions::default`] permits nothing at all, which is the only safe
/// starting point for code nobody has read.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppPermissions {
    /// Filesystem rules. Empty means no filesystem access.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub filesystem: Vec<FilesystemRule>,

    /// Network policy. Denies everything by default.
    pub network: NetworkPolicy,

    /// Process policy. Denies everything by default.
    pub process: ProcessPolicy,

    /// Device policy. Denies everything by default.
    pub devices: DevicePolicy,

    /// Names of environment settings the application may use. Values live in
    /// secure storage and never appear here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub environment: Vec<String>,
}

impl AppPermissions {
    /// An application that may do nothing.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Records that an application *asks* for a capability.
    ///
    /// A request, never a grant. This block is the application's statement of
    /// what it wants; whether it gets any of it is decided by a person and
    /// recorded in the ledger, which this cannot reach.
    ///
    /// Adding the same request twice is idempotent — a manifest listing a
    /// capability twice should not read as wanting it more.
    pub fn request(&mut self, permission: &AppPermission) {
        match permission {
            AppPermission::FilesystemRead { scope } => {
                self.add_rule(FilesystemRule::Read(scope.clone()));
            }
            AppPermission::FilesystemWrite { scope } => {
                self.add_rule(FilesystemRule::Write(scope.clone()));
            }
            AppPermission::NetworkOutbound { scope } => {
                self.network.outbound = true;
                if !self.network.allowed_hosts.contains(scope) {
                    self.network.allowed_hosts.push(scope.clone());
                }
            }
            AppPermission::NetworkInbound { port } => {
                if !self.network.inbound_ports.contains(port) {
                    self.network.inbound_ports.push(*port);
                }
            }
            AppPermission::ReadEnvironment { name } => {
                if !self.environment.contains(name) {
                    self.environment.push(name.clone());
                }
            }
            AppPermission::ExecuteProcesses => self.process.execute = true,
            AppPermission::Camera => self.devices.camera = true,
            AppPermission::Microphone => self.devices.microphone = true,
            AppPermission::Location => self.devices.location = true,
            // No catch-all: a new capability should break this match rather
            // than be silently dropped. A request nobody recorded is a request
            // that cannot be granted, which sounds safe until it is the reason
            // an application does not work and nothing says why.
        }
    }

    /// Adds a filesystem rule if an identical one is not already present.
    fn add_rule(&mut self, rule: FilesystemRule) {
        if !self.filesystem.contains(&rule) {
            self.filesystem.push(rule);
        }
    }

    /// The flat capability list the ledger stores and checks.
    ///
    /// The single conversion point between what a person reads and what the
    /// system enforces.
    #[must_use]
    pub fn capabilities(&self) -> Vec<AppPermission> {
        let mut capabilities: Vec<AppPermission> = self
            .filesystem
            .iter()
            .flat_map(FilesystemRule::capabilities)
            .collect();

        if self.network.outbound {
            capabilities.extend(
                self.network
                    .allowed_hosts
                    .iter()
                    .cloned()
                    .map(AppPermission::outbound),
            );
        }
        capabilities.extend(
            self.network
                .inbound_ports
                .iter()
                .map(|port| AppPermission::NetworkInbound { port: *port }),
        );

        if self.process.execute {
            capabilities.push(AppPermission::ExecuteProcesses);
        }
        if self.devices.camera {
            capabilities.push(AppPermission::Camera);
        }
        if self.devices.microphone {
            capabilities.push(AppPermission::Microphone);
        }
        if self.devices.location {
            capabilities.push(AppPermission::Location);
        }

        capabilities.extend(
            self.environment
                .iter()
                .map(|name| AppPermission::ReadEnvironment { name: name.clone() }),
        );

        capabilities
    }

    /// Whether this application asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.capabilities().is_empty()
    }

    /// The highest risk level among the requested capabilities, if any.
    ///
    /// Used to decide how emphatically to ask.
    #[must_use]
    pub fn highest_risk(&self) -> Option<RiskLevel> {
        self.capabilities().iter().map(AppPermission::risk).max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    fn host(name: &str) -> HostScope {
        HostScope::parse(name).unwrap()
    }

    /// The permission set from the product brief's example.
    fn apartment_comparator() -> AppPermissions {
        AppPermissions {
            filesystem: vec![FilesystemRule::Read(scope("~/Downloads/apartments/**"))],
            ..AppPermissions::none()
        }
    }

    // --- the default is nothing ---------------------------------------------

    /// The single most important property in this file. Code nobody has read
    /// starts with no capabilities whatsoever.
    #[test]
    fn an_application_starts_with_nothing() {
        let nothing = AppPermissions::none();
        assert!(nothing.is_empty());
        assert_eq!(nothing.capabilities(), Vec::new());
        assert_eq!(nothing.highest_risk(), None);
        assert!(!nothing.network.outbound);
        assert!(!nothing.process.execute);
        assert!(!nothing.devices.camera);
    }

    /// An empty manifest block must deserialise to no permissions, not to
    /// defaults that happen to be permissive.
    #[test]
    fn an_empty_manifest_block_grants_nothing() {
        let parsed: AppPermissions = serde_norway::from_str("{}").unwrap();
        assert_eq!(parsed, AppPermissions::none());

        let parsed: AppPermissions =
            serde_norway::from_str("network:\n  outbound: true\n").unwrap();
        assert!(
            parsed.capabilities().is_empty(),
            "enabling outbound without naming a destination must grant nothing"
        );
    }

    // --- declarative form to enforced form -----------------------------------

    #[test]
    fn the_readable_form_and_the_enforced_form_agree() {
        let capabilities = apartment_comparator().capabilities();
        assert_eq!(
            capabilities,
            vec![AppPermission::read(scope("~/Downloads/apartments/**"))]
        );
    }

    #[test]
    fn read_write_expands_into_both_capabilities() {
        let permissions = AppPermissions {
            filesystem: vec![FilesystemRule::ReadWrite(scope("~/reports/**"))],
            ..AppPermissions::none()
        };
        let capabilities = permissions.capabilities();

        assert!(capabilities.contains(&AppPermission::read(scope("~/reports/**"))));
        assert!(capabilities.contains(&AppPermission::write(scope("~/reports/**"))));
        assert_eq!(capabilities.len(), 2);
    }

    #[test]
    fn every_declarative_switch_produces_its_capability() {
        let permissions = AppPermissions {
            filesystem: vec![],
            network: NetworkPolicy {
                outbound: true,
                allowed_hosts: vec![host("api.example.com")],
                inbound_ports: vec![8080],
            },
            process: ProcessPolicy { execute: true },
            devices: DevicePolicy {
                camera: true,
                microphone: true,
                location: true,
            },
            environment: vec!["API_KEY".to_owned()],
        };
        let capabilities = permissions.capabilities();

        for expected in [
            AppPermission::outbound(host("api.example.com")),
            AppPermission::NetworkInbound { port: 8080 },
            AppPermission::ExecuteProcesses,
            AppPermission::Camera,
            AppPermission::Microphone,
            AppPermission::Location,
            AppPermission::ReadEnvironment {
                name: "API_KEY".to_owned(),
            },
        ] {
            assert!(
                capabilities.contains(&expected),
                "{expected} was not produced"
            );
        }
    }

    // --- the manifest format -------------------------------------------------

    #[test]
    fn the_manifest_form_is_readable_yaml() {
        let yaml = serde_norway::to_string(&apartment_comparator()).unwrap();
        assert!(
            yaml.contains("- read: ~/Downloads/apartments/**"),
            "a filesystem rule should read as one line a person can check:\n{yaml}"
        );

        let parsed: AppPermissions = serde_norway::from_str(&yaml).unwrap();
        assert_eq!(parsed, apartment_comparator());
    }

    /// A typo in a manifest must be an error, not a silently ignored key that
    /// leaves the user believing they restricted something.
    #[test]
    fn unknown_manifest_keys_are_refused() {
        assert!(serde_norway::from_str::<AppPermissions>("netwrok:\n  outbound: true\n").is_err());
        assert!(serde_norway::from_str::<AppPermissions>("process:\n  exectue: true\n").is_err());
    }

    // --- satisfaction --------------------------------------------------------

    #[test]
    fn scoped_capabilities_satisfy_by_containment() {
        let held = AppPermission::read(scope("~/Downloads/apartments/**"));

        assert!(held.satisfies(&AppPermission::read(scope("~/Downloads/apartments/a.csv"))));
        assert!(!held.satisfies(&AppPermission::read(scope("~/Downloads/taxes/a.csv"))));
        assert!(!held.satisfies(&AppPermission::write(scope("~/Downloads/apartments/a.csv"))));
    }

    #[test]
    fn network_capabilities_satisfy_by_host_containment() {
        let held = AppPermission::outbound(host("*.example.com"));

        assert!(held.satisfies(&AppPermission::outbound(host("api.example.com"))));
        assert!(!held.satisfies(&AppPermission::outbound(host("example.com"))));
        assert!(!held.satisfies(&AppPermission::outbound(host("attacker.net"))));
    }

    #[test]
    fn environment_and_port_capabilities_match_exactly() {
        let env = AppPermission::ReadEnvironment {
            name: "API_KEY".to_owned(),
        };
        assert!(env.satisfies(&env));
        assert!(!env.satisfies(&AppPermission::ReadEnvironment {
            name: "OTHER_KEY".to_owned()
        }));

        let port = AppPermission::NetworkInbound { port: 8080 };
        assert!(port.satisfies(&port));
        assert!(!port.satisfies(&AppPermission::NetworkInbound { port: 8081 }));
    }

    // --- the ceiling rule ----------------------------------------------------

    /// Every app capability names the meta-permission Ephemeral must also hold,
    /// so revoking a meta-permission disables that capability product-wide.
    #[test]
    fn every_capability_names_the_meta_permission_it_needs() {
        let cases = [
            (
                AppPermission::read(scope("~/a/**")),
                MetaPermission::read(scope("~/a/**")),
            ),
            (
                AppPermission::write(scope("~/a/**")),
                MetaPermission::write(scope("~/a/**")),
            ),
            (
                AppPermission::outbound(host("api.example.com")),
                MetaPermission::NetworkAccess,
            ),
            (
                AppPermission::NetworkInbound { port: 80 },
                MetaPermission::NetworkAccess,
            ),
            (
                AppPermission::ExecuteProcesses,
                MetaPermission::ExecuteProcesses,
            ),
            (AppPermission::Camera, MetaPermission::Camera),
            (AppPermission::Microphone, MetaPermission::Microphone),
            (AppPermission::Location, MetaPermission::Location),
            (
                AppPermission::ReadEnvironment {
                    name: "API_KEY".to_owned(),
                },
                MetaPermission::ReadEnvironment,
            ),
        ];

        for (app, expected_meta) in cases {
            assert_eq!(
                app.required_meta(),
                expected_meta,
                "{app} named the wrong meta-permission"
            );
        }
    }

    // --- risk and explanations ----------------------------------------------

    #[test]
    fn every_capability_explains_itself_and_its_consequences() {
        let everything = AppPermissions {
            filesystem: vec![FilesystemRule::ReadWrite(scope("~/a/**"))],
            network: NetworkPolicy {
                outbound: true,
                allowed_hosts: vec![host("*"), host("api.example.com")],
                inbound_ports: vec![8080],
            },
            process: ProcessPolicy { execute: true },
            devices: DevicePolicy {
                camera: true,
                microphone: true,
                location: true,
            },
            environment: vec!["API_KEY".to_owned()],
        };

        for permission in everything.capabilities() {
            assert!(
                permission.describe().len() > 5,
                "{permission} has no description"
            );
            assert!(
                permission.consequences().len() > 20,
                "{permission} has no usable consequences text"
            );
        }
    }

    /// Unrestricted egress is the capability that turns a read permission into
    /// a data-exfiltration permission, so it must be flagged accordingly.
    #[test]
    fn unrestricted_egress_is_high_risk_and_says_why() {
        let anywhere = AppPermission::outbound(host("*"));
        assert_eq!(anywhere.risk(), RiskLevel::High);
        assert!(anywhere.consequences().contains("read from your files"));

        assert!(
            AppPermission::outbound(host("api.example.com")).risk() < anywhere.risk(),
            "a named destination must be less risky than anywhere"
        );
    }

    #[test]
    fn running_other_programs_is_critical_for_a_generated_app() {
        assert_eq!(AppPermission::ExecuteProcesses.risk(), RiskLevel::Critical);
        assert!(
            AppPermission::ExecuteProcesses
                .risk()
                .requires_explicit_confirmation()
        );
    }

    #[test]
    fn highest_risk_reports_the_worst_capability_requested() {
        assert_eq!(
            apartment_comparator().highest_risk(),
            Some(RiskLevel::Medium)
        );

        let dangerous = AppPermissions {
            process: ProcessPolicy { execute: true },
            ..apartment_comparator()
        };
        assert_eq!(dangerous.highest_risk(), Some(RiskLevel::Critical));
    }

    #[test]
    fn app_permissions_round_trip_through_json() {
        for permission in [
            AppPermission::read(scope("~/a/**")),
            AppPermission::outbound(host("api.example.com:443")),
            AppPermission::NetworkInbound { port: 8080 },
            AppPermission::ExecuteProcesses,
            AppPermission::ReadEnvironment {
                name: "API_KEY".to_owned(),
            },
        ] {
            let json = serde_json::to_string(&permission).unwrap();
            assert_eq!(
                serde_json::from_str::<AppPermission>(&json).unwrap(),
                permission
            );
        }
    }
}
