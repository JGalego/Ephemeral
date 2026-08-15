//! Meta-permissions: what **Ephemeral itself** is allowed to do.
//!
//! These are the product's own capabilities — running Docker, installing
//! runtimes, executing processes, reaching the network, touching the keychain.
//! They are granted to [`Principal::Ephemeral`](crate::identity::Principal)
//! and to nobody else.
//!
//! A meta-permission is **never** a source of authority for a generated
//! application. Ephemeral holding `filesystem.read(~/**)` does not let a
//! generated app read a single file. It is, however, a *ceiling*: an app can
//! only do something if both it and Ephemeral are permitted, so revoking a
//! meta-permission disables that capability product-wide ([ADR-0003]).
//!
//! [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{PathScope, RiskLevel};

/// Something Ephemeral itself may be allowed to do.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MetaPermission {
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

    /// Run processes on the host.
    ExecuteProcesses,

    /// Install runtimes, interpreters and other dependencies applications need.
    InstallDependencies,

    /// Make outbound network connections — to model providers, package
    /// registries and container registries.
    NetworkAccess,

    /// Use an installed container runtime.
    UseDocker,

    /// Install a container runtime that is not present.
    ///
    /// Separate from [`MetaPermission::UseDocker`] because installing one is a
    /// privileged, system-wide change and using one is not ([ADR-0005]).
    ///
    /// [ADR-0005]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0005-docker-first-runtime-abstraction.md
    InstallDocker,

    /// Pull container images from a registry.
    PullImages,

    /// Read Ephemeral's own environment variables.
    ReadEnvironment,

    /// Read and write entries in the operating system's keychain or credential
    /// store.
    AccessKeychain,

    /// Use stored credentials — provider API keys and similar.
    AccessCredentials,

    /// Create desktop shortcuts and launcher entries.
    CreateShortcuts,

    /// Send desktop or mobile notifications.
    SendNotifications,

    /// Use the microphone.
    Microphone,

    /// Use the camera.
    Camera,

    /// Read the device's location.
    Location,

    /// Read the address book.
    Contacts,

    /// Read the calendar.
    Calendar,

    /// Read browser history, bookmarks or cookies.
    BrowserData,

    /// Talk to connected external devices.
    ExternalDevices,

    /// Update or modify Ephemeral's own installation.
    SelfUpdate,
}

impl MetaPermission {
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

    /// The stable capability name, matching the serialised form.
    #[must_use]
    pub fn capability(&self) -> &'static str {
        match self {
            Self::FilesystemRead { .. } => "filesystem_read",
            Self::FilesystemWrite { .. } => "filesystem_write",
            Self::ExecuteProcesses => "execute_processes",
            Self::InstallDependencies => "install_dependencies",
            Self::NetworkAccess => "network_access",
            Self::UseDocker => "use_docker",
            Self::InstallDocker => "install_docker",
            Self::PullImages => "pull_images",
            Self::ReadEnvironment => "read_environment",
            Self::AccessKeychain => "access_keychain",
            Self::AccessCredentials => "access_credentials",
            Self::CreateShortcuts => "create_shortcuts",
            Self::SendNotifications => "send_notifications",
            Self::Microphone => "microphone",
            Self::Camera => "camera",
            Self::Location => "location",
            Self::Contacts => "contacts",
            Self::Calendar => "calendar",
            Self::BrowserData => "browser_data",
            Self::ExternalDevices => "external_devices",
            Self::SelfUpdate => "self_update",
        }
    }

    /// Whether a grant of `self` covers a request for `requested`.
    ///
    /// Scoped capabilities compare their scopes; unscoped ones compare by
    /// identity. Two *different* capabilities never satisfy each other, however
    /// similar they look — reading is not writing, and using Docker is not
    /// installing it.
    #[must_use]
    pub fn satisfies(&self, requested: &Self) -> bool {
        match (self, requested) {
            (Self::FilesystemRead { scope: held }, Self::FilesystemRead { scope: want })
            | (Self::FilesystemWrite { scope: held }, Self::FilesystemWrite { scope: want }) => {
                held.contains(want)
            }
            (held, want) => held.capability() == want.capability(),
        }
    }

    /// How dangerous this capability is.
    ///
    /// Anything [`RiskLevel::High`] or above needs an explicit, unambiguous
    /// confirmation rather than a default-highlighted button.
    #[must_use]
    pub fn risk(&self) -> RiskLevel {
        match self {
            // Modifying Ephemeral itself, or gaining arbitrary code execution,
            // undermines every other control in the product.
            Self::SelfUpdate | Self::ExecuteProcesses | Self::InstallDocker => RiskLevel::Critical,

            // Credentials, whole-root filesystem access and personal data.
            Self::AccessCredentials
            | Self::AccessKeychain
            | Self::BrowserData
            | Self::Contacts
            | Self::Calendar
            | Self::Location
            | Self::Camera
            | Self::Microphone
            | Self::ExternalDevices
            | Self::InstallDependencies => RiskLevel::High,

            Self::FilesystemWrite { scope } => {
                if scope.is_whole_root() {
                    RiskLevel::Critical
                } else {
                    RiskLevel::High
                }
            }
            Self::FilesystemRead { scope } => {
                if scope.is_whole_root() {
                    RiskLevel::High
                } else {
                    RiskLevel::Medium
                }
            }

            Self::NetworkAccess | Self::UseDocker | Self::PullImages | Self::ReadEnvironment => {
                RiskLevel::Medium
            }

            Self::CreateShortcuts | Self::SendNotifications => RiskLevel::Low,
        }
    }

    /// What this permission lets Ephemeral do, in plain language.
    ///
    /// Phrased to complete the sentence "Ephemeral wants to …".
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::FilesystemRead { scope } => {
                if scope.is_whole_root() {
                    format!("read any file under {}", scope.display_path())
                } else if scope.is_recursive() {
                    format!("read the files in {}", scope.display_path())
                } else {
                    format!("read {}", scope.display_path())
                }
            }
            Self::FilesystemWrite { scope } => {
                if scope.is_recursive() {
                    format!(
                        "create, change and delete files in {}",
                        scope.display_path()
                    )
                } else {
                    format!("create, change and delete {}", scope.display_path())
                }
            }
            Self::ExecuteProcesses => "run programs on this device".to_owned(),
            Self::InstallDependencies => "install the runtimes and tools your apps need".to_owned(),
            Self::NetworkAccess => "connect to the internet".to_owned(),
            Self::UseDocker => "use Docker to run your apps in containers".to_owned(),
            Self::InstallDocker => "install Docker on this device".to_owned(),
            Self::PullImages => "download container images".to_owned(),
            Self::ReadEnvironment => "read its own environment variables".to_owned(),
            Self::AccessKeychain => {
                "store and read secrets in this device's secure storage".to_owned()
            }
            Self::AccessCredentials => "use the credentials you have saved".to_owned(),
            Self::CreateShortcuts => "add shortcuts for your apps".to_owned(),
            Self::SendNotifications => "send you notifications".to_owned(),
            Self::Microphone => "use the microphone".to_owned(),
            Self::Camera => "use the camera".to_owned(),
            Self::Location => "read this device's location".to_owned(),
            Self::Contacts => "read your contacts".to_owned(),
            Self::Calendar => "read your calendar".to_owned(),
            Self::BrowserData => "read your browser history and bookmarks".to_owned(),
            Self::ExternalDevices => "talk to devices connected to this one".to_owned(),
            Self::SelfUpdate => "update and modify itself".to_owned(),
        }
    }

    /// What allowing this permission actually means, and what it still does not
    /// permit.
    ///
    /// The second half matters: a permission prompt that only says what is
    /// gained invites people to imagine the worst.
    #[must_use]
    pub fn consequences(&self) -> String {
        let generated_apps_note = "Generated apps get none of this — they have their own, \
             separate permissions.";

        let specific = match self {
            Self::FilesystemRead { scope } => format!(
                "Ephemeral can read what is at {}. It cannot write there, and it cannot \
                 read anywhere else.",
                scope.display_path()
            ),
            Self::FilesystemWrite { scope } => format!(
                "Ephemeral can add, change and remove files at {}. It cannot touch \
                 anything outside it.",
                scope.display_path()
            ),
            Self::ExecuteProcesses => {
                "Ephemeral can run programs as you. This is a broad capability and is \
                 what makes building and testing apps possible."
                    .to_owned()
            }
            Self::InstallDocker => {
                "Ephemeral can install Docker, which is a system-wide change and may ask \
                 for your administrator password."
                    .to_owned()
            }
            Self::UseDocker => {
                "Ephemeral can create and destroy containers for your apps. Each app is \
                 still confined to what that app is permitted."
                    .to_owned()
            }
            Self::NetworkAccess => {
                "Ephemeral can reach the internet — to talk to your AI provider and to \
                 download what your apps need."
                    .to_owned()
            }
            Self::AccessCredentials | Self::AccessKeychain => {
                "Ephemeral can read and store secrets in this device's secure storage. \
                 Secret values are never written into an app, a manifest, or a log."
                    .to_owned()
            }
            Self::SelfUpdate => {
                "Ephemeral can change its own installation. This is the most powerful \
                 permission it can hold."
                    .to_owned()
            }
            other => format!("Ephemeral can {}.", other.describe()),
        };

        format!("{specific} {generated_apps_note}")
    }
}

impl fmt::Display for MetaPermission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FilesystemRead { scope } | Self::FilesystemWrite { scope } => {
                write!(f, "{}({scope})", self.capability())
            }
            other => f.write_str(other.capability()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    /// Every capability listed in the product brief must be expressible, or
    /// Ephemeral would be doing something it cannot ask about.
    fn every_capability() -> Vec<MetaPermission> {
        vec![
            MetaPermission::ExecuteProcesses,
            MetaPermission::InstallDependencies,
            MetaPermission::NetworkAccess,
            MetaPermission::UseDocker,
            MetaPermission::InstallDocker,
            MetaPermission::PullImages,
            MetaPermission::ReadEnvironment,
            MetaPermission::AccessKeychain,
            MetaPermission::AccessCredentials,
            MetaPermission::CreateShortcuts,
            MetaPermission::SendNotifications,
            MetaPermission::Microphone,
            MetaPermission::Camera,
            MetaPermission::Location,
            MetaPermission::Contacts,
            MetaPermission::Calendar,
            MetaPermission::BrowserData,
            MetaPermission::ExternalDevices,
            MetaPermission::SelfUpdate,
            MetaPermission::read(scope("~/**")),
            MetaPermission::write(scope("~/**")),
        ]
    }

    #[test]
    fn every_capability_explains_itself() {
        for permission in every_capability() {
            assert!(
                permission.describe().len() > 5,
                "{permission} has no usable description"
            );
            assert!(
                permission.consequences().contains("separate permissions"),
                "{permission} must say that generated apps do not inherit this"
            );
            assert!(!permission.capability().is_empty());
        }
    }

    #[test]
    fn capability_names_are_unique() {
        let all = every_capability();
        let mut names: Vec<_> = all.iter().map(MetaPermission::capability).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    /// A grant on a directory tree satisfies a request for anything inside it,
    /// and nothing outside it.
    #[test]
    fn scoped_capabilities_satisfy_by_containment() {
        let held = MetaPermission::read(scope("~/Downloads/**"));

        assert!(held.satisfies(&MetaPermission::read(scope("~/Downloads/a.csv"))));
        assert!(!held.satisfies(&MetaPermission::read(scope("~/Documents/a.csv"))));
        assert!(!held.satisfies(&MetaPermission::read(scope("~/**"))));
    }

    /// Reading is not writing. A capability never satisfies a different one,
    /// however similar the scope.
    #[test]
    fn reading_never_satisfies_writing() {
        let read = MetaPermission::read(scope("~/**"));
        let write = MetaPermission::write(scope("~/Downloads/a.csv"));

        assert!(!read.satisfies(&write));
        assert!(!write.satisfies(&read));
    }

    #[test]
    fn unscoped_capabilities_satisfy_only_themselves() {
        assert!(MetaPermission::UseDocker.satisfies(&MetaPermission::UseDocker));
        assert!(!MetaPermission::UseDocker.satisfies(&MetaPermission::InstallDocker));
        assert!(!MetaPermission::InstallDocker.satisfies(&MetaPermission::UseDocker));
        assert!(!MetaPermission::NetworkAccess.satisfies(&MetaPermission::PullImages));
    }

    /// The capabilities that can undo every other control must demand an
    /// explicit confirmation.
    #[test]
    fn the_most_dangerous_capabilities_are_marked_critical() {
        for permission in [
            MetaPermission::SelfUpdate,
            MetaPermission::ExecuteProcesses,
            MetaPermission::InstallDocker,
            MetaPermission::write(scope("~/**")),
        ] {
            assert_eq!(
                permission.risk(),
                RiskLevel::Critical,
                "{permission} should be critical"
            );
            assert!(permission.risk().requires_explicit_confirmation());
        }
    }

    #[test]
    fn a_narrow_scope_is_less_risky_than_a_whole_root() {
        assert!(
            MetaPermission::read(scope("~/Downloads/**")).risk()
                < MetaPermission::read(scope("~/**")).risk()
        );
        assert!(
            MetaPermission::read(scope("~/**")).risk()
                < MetaPermission::write(scope("~/**")).risk()
        );
    }

    #[test]
    fn meta_permissions_round_trip_through_json() {
        for permission in every_capability() {
            let json = serde_json::to_string(&permission).unwrap();
            assert_eq!(
                serde_json::from_str::<MetaPermission>(&json).unwrap(),
                permission,
                "{json} did not round-trip"
            );
            assert!(json.contains(permission.capability()));
        }
    }
}
