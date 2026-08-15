//! Where an application's files live, and everything else worth recording about
//! it.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::ManifestError;
use crate::retention::RetentionPolicy;

/// Where a generated application actually executes.
///
/// Surfaced in the interface rather than treated as an implementation detail: if
/// an application runs on a server, the user's data goes to a server, and that
/// is the most important thing to know before handing over a file ([ADR-0007]).
///
/// [ADR-0007]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0007-mobile-control-plane.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "where", rename_all = "snake_case")]
pub enum ExecutionLocation {
    /// On the user's own device.
    Local,

    /// On a control plane, reached over the network.
    Remote {
        /// Which control plane, so the user can see where their data goes.
        control_plane: String,
    },
}

impl Default for ExecutionLocation {
    /// Local. Desktop Ephemeral is local-first, and remote execution is
    /// something a user opts into knowingly.
    fn default() -> Self {
        Self::Local
    }
}

impl ExecutionLocation {
    /// Whether the application runs on this device.
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    /// What to tell the user about where this runs.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Local => "This app runs on your device. Nothing it reads leaves it.".to_owned(),
            Self::Remote { control_plane } => format!(
                "This app runs on {control_plane}, not on your device. Files and data you \
                 give it are sent there."
            ),
        }
    }
}

impl fmt::Display for ExecutionLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Remote { control_plane } => write!(f, "remote({control_plane})"),
        }
    }
}

/// Where an application's files live, relative to its own storage directory.
///
/// Paths are **relative and contained**: they name a location inside
/// `<data-root>/apps/<app-id>/` and nowhere else. That is checked by
/// [`Artifacts::validate`] on every load, because a manifest is a document a
/// user can edit and an attacker might supply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Artifacts {
    /// The generated source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Build output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,

    /// Build, test and runtime logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,

    /// Exports and reports the application produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports: Option<String>,
}

impl Artifacts {
    /// The conventional layout, matching the storage hierarchy.
    #[must_use]
    pub fn conventional() -> Self {
        Self {
            source: Some("source".to_owned()),
            build: Some("build".to_owned()),
            logs: Some("logs".to_owned()),
            exports: Some("artifacts".to_owned()),
        }
    }

    /// Checks that every path stays inside the application's own directory.
    ///
    /// # Errors
    ///
    /// [`ManifestError::InvalidField`] if a path is absolute, contains a `..`
    /// segment, or contains a NUL byte — the three ways a relative path escapes.
    pub fn validate(&self) -> Result<(), ManifestError> {
        for (field, path) in [
            ("artifacts.source", &self.source),
            ("artifacts.build", &self.build),
            ("artifacts.logs", &self.logs),
            ("artifacts.exports", &self.exports),
        ] {
            let Some(path) = path else { continue };
            check_contained(field, path)?;
        }
        Ok(())
    }
}

/// Refuses anything that would leave the application's directory.
fn check_contained(field: &'static str, path: &str) -> Result<(), ManifestError> {
    let problem = if path.trim().is_empty() {
        Some("is empty")
    } else if path.contains('\0') {
        Some("contains a NUL byte")
    } else if path.starts_with('/') || path.starts_with('\\') || path.starts_with('~') {
        Some("must be relative to the application's own directory")
    } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
        Some("must not name a drive")
    } else if path
        .split(['/', '\\'])
        .any(|segment| segment == ".." || segment == "~")
    {
        Some("must not contain a '..' segment")
    } else {
        None
    };

    match problem {
        Some(problem) => Err(ManifestError::InvalidField {
            field,
            problem: problem.to_owned(),
        }),
        None => Ok(()),
    }
}

/// Everything else worth recording about an application.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Metadata {
    /// Free-form tags for grouping and search.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// What the user actually wanted, in their own words.
    ///
    /// The durable object in Ephemeral is the *intent*; the generated
    /// application is disposable implementation detail. This field is the
    /// intent, which is why it survives regeneration and repair.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub purpose: String,

    /// How ephemeral this application is.
    pub retention: RetentionPolicy,

    /// Where it executes.
    pub execution: ExecutionLocation,
}

impl Metadata {
    /// Metadata for an application created from a user's request.
    #[must_use]
    pub fn for_intent(purpose: impl Into<String>, retention: RetentionPolicy) -> Self {
        Self {
            tags: Vec::new(),
            purpose: purpose.into(),
            retention,
            execution: ExecutionLocation::Local,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- execution location ----------------------------------------------------

    #[test]
    fn execution_is_local_by_default() {
        assert!(ExecutionLocation::default().is_local());
        assert!(Metadata::default().execution.is_local());
    }

    /// Remote execution must be stated plainly, and must name where the data
    /// goes. "Runs in the cloud" is not an answer a person can act on.
    #[test]
    fn remote_execution_names_where_the_data_goes() {
        let remote = ExecutionLocation::Remote {
            control_plane: "ephemeral.example.com".to_owned(),
        };

        assert!(!remote.is_local());
        let described = remote.describe();
        assert!(described.contains("ephemeral.example.com"));
        assert!(described.contains("not on your device"));

        assert!(ExecutionLocation::Local.describe().contains("leaves it"));
    }

    // --- artifact containment: security-relevant --------------------------------

    #[test]
    fn the_conventional_layout_is_valid() {
        Artifacts::conventional().validate().unwrap();
        Artifacts::default().validate().unwrap();
    }

    #[test]
    fn nested_relative_paths_are_fine() {
        let artifacts = Artifacts {
            source: Some("source/app".to_owned()),
            build: Some("build/dist".to_owned()),
            ..Artifacts::default()
        };
        artifacts.validate().unwrap();
    }

    /// An artifact path is joined onto the application's storage directory, so
    /// anything that could climb out of it is refused at load.
    #[test]
    fn artifact_paths_cannot_escape_the_application_directory() {
        for hostile in [
            "/etc/passwd",
            "~/.ssh",
            "../other-app/source",
            "source/../../other-app",
            "C:/Windows",
            "\\\\server\\share",
            "..",
            "",
            "sou\0rce",
        ] {
            let artifacts = Artifacts {
                source: Some(hostile.to_owned()),
                ..Artifacts::default()
            };
            assert!(
                artifacts.validate().is_err(),
                "{hostile:?} must be refused: it is joined onto the app's directory"
            );
        }
    }

    #[test]
    fn every_artifact_field_is_checked_not_just_the_first() {
        for artifacts in [
            Artifacts {
                build: Some("../escape".to_owned()),
                ..Artifacts::conventional()
            },
            Artifacts {
                logs: Some("/var/log".to_owned()),
                ..Artifacts::conventional()
            },
            Artifacts {
                exports: Some("~/Desktop".to_owned()),
                ..Artifacts::conventional()
            },
        ] {
            assert!(artifacts.validate().is_err());
        }
    }

    // --- round trips ------------------------------------------------------------

    #[test]
    fn metadata_round_trips_through_yaml() {
        let metadata = Metadata {
            tags: vec!["csv".to_owned(), "one-off".to_owned()],
            purpose: "Compare the two listing exports I downloaded.".to_owned(),
            retention: RetentionPolicy::default(),
            execution: ExecutionLocation::Remote {
                control_plane: "ephemeral.example.com".to_owned(),
            },
        };

        let yaml = serde_norway::to_string(&metadata).unwrap();
        assert_eq!(serde_norway::from_str::<Metadata>(&yaml).unwrap(), metadata);
    }

    #[test]
    fn a_typo_in_a_metadata_block_is_an_error() {
        assert!(serde_norway::from_str::<Metadata>("porpoise: compare files\n").is_err());
        assert!(serde_norway::from_str::<Artifacts>("sauce: source\n").is_err());
    }
}
