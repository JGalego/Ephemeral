//! Identifiers, and the principals that can hold a permission.
//!
//! An [`AppId`] is not just a name. It is used as a filesystem path component
//! (`<data-root>/apps/<app-id>/`), as a container label, and as the subject of
//! every permission grant. Those three uses make it a security-relevant value:
//! an id containing `..` or a path separator would let one application's storage
//! escape into another's, and an id that collides would merge two principals.
//!
//! So [`AppId`] is validated on construction and there is no way to build one
//! that skips validation.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The maximum length of an application identifier.
///
/// Comfortably below the path-component limit on every target platform, with
/// room for the storage layout's subdirectories.
pub const MAX_ID_LENGTH: usize = 64;

/// Why an identifier was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// The identifier was empty.
    #[error("identifier is empty")]
    Empty,

    /// The identifier was longer than [`MAX_ID_LENGTH`].
    #[error("identifier is {length} characters, the maximum is {MAX_ID_LENGTH}")]
    TooLong {
        /// The length that was rejected.
        length: usize,
    },

    /// The identifier contained a character outside the permitted set.
    ///
    /// This is the check that stops `..`, `/`, `\`, NUL and similar from
    /// reaching a path join.
    #[error(
        "identifier {identifier:?} contains {character:?}; only lowercase letters, \
         digits and hyphens are allowed"
    )]
    InvalidCharacter {
        /// The rejected identifier.
        identifier: String,
        /// The first offending character.
        character: char,
    },

    /// The identifier started or ended with a hyphen, or contained a run of
    /// them.
    ///
    /// Rejected so that identifiers stay readable and round-trip through
    /// slugification unchanged.
    #[error("identifier {identifier:?} must not start, end, or repeat a hyphen")]
    MalformedHyphens {
        /// The rejected identifier.
        identifier: String,
    },
}

/// The identity of a generated application.
///
/// Guaranteed to be a non-empty, lowercase `[a-z0-9-]` string of at most
/// [`MAX_ID_LENGTH`] characters, with no leading, trailing or repeated hyphens.
/// That makes it safe to use as a single path component and as a container
/// label.
///
/// Construct one with [`AppId::parse`] (validating) or [`AppId::generate`]
/// (derives a unique id from a human name).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppId(String);

impl AppId {
    /// Validates and wraps an identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] if the identifier is empty, too long, contains a
    /// character outside `[a-z0-9-]`, or is malformed with respect to hyphens.
    pub fn parse(id: impl Into<String>) -> Result<Self, IdError> {
        let id = id.into();

        if id.is_empty() {
            return Err(IdError::Empty);
        }
        if id.len() > MAX_ID_LENGTH {
            return Err(IdError::TooLong { length: id.len() });
        }
        if let Some(character) = id
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
        {
            return Err(IdError::InvalidCharacter {
                identifier: id,
                character,
            });
        }
        if id.starts_with('-') || id.ends_with('-') || id.contains("--") {
            return Err(IdError::MalformedHyphens { identifier: id });
        }

        Ok(Self(id))
    }

    /// A stable identifier derived from a name alone.
    ///
    /// No random suffix, so the same name always yields the same id. Used where
    /// an id must not carry information about *this* installation — publishing a
    /// package, above all, where the sender's own id would otherwise travel and
    /// two publishes of the same application would differ for no reason.
    ///
    /// Not for creating applications: two on one machine could collide.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let slug = slugify(name);
        let stem: String = slug.chars().take(MAX_ID_LENGTH).collect();
        let stem = stem.trim_end_matches('-');

        Self(if stem.is_empty() {
            "app".to_owned()
        } else {
            stem.to_owned()
        })
    }

    /// Derives a unique identifier from a human-readable name.
    ///
    /// The name is slugified and a short random suffix is appended, so two apps
    /// called "CSV Comparator" get distinct ids and neither can be predicted by
    /// a third party. A name that slugifies to nothing still yields a valid id.
    #[must_use]
    pub fn generate(name: &str) -> Self {
        let slug = slugify(name);
        let suffix = short_random_suffix();

        // Leave room for the separator and the suffix.
        let budget = MAX_ID_LENGTH - suffix.len() - 1;
        let stem = if slug.is_empty() {
            "app".to_owned()
        } else {
            slug.chars().take(budget).collect::<String>()
        };
        let stem = stem.trim_end_matches('-');
        let stem = if stem.is_empty() { "app" } else { stem };

        // Construction is by the same rules `parse` enforces, so this cannot
        // produce an invalid id.
        Self(format!("{stem}-{suffix}"))
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for AppId {
    type Error = IdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<AppId> for String {
    fn from(value: AppId) -> Self {
        value.0
    }
}

impl std::str::FromStr for AppId {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The identity of a plugin.
///
/// Reserved for the future plugin architecture. Plugins are principals in their
/// own right: installing one grants it nothing (`ARCHITECTURE.md` §12).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PluginId(String);

impl PluginId {
    /// Validates and wraps a plugin identifier, using the same rules as
    /// [`AppId`].
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] under the same conditions as [`AppId::parse`].
    pub fn parse(id: impl Into<String>) -> Result<Self, IdError> {
        AppId::parse(id).map(|valid| Self(valid.0))
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for PluginId {
    type Error = IdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<PluginId> for String {
    fn from(value: PluginId) -> Self {
        value.0
    }
}

/// Something that can hold a permission.
///
/// Principals are isolated from one another. A grant names exactly one
/// principal, and **no principal inherits another's grants** — in particular a
/// generated application inherits nothing from [`Principal::Ephemeral`]. That
/// is the central invariant of the permission model ([ADR-0003]).
///
/// [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Principal {
    /// Ephemeral itself. Holds meta-permissions; grants nothing to anyone else.
    Ephemeral,

    /// One generated application.
    App {
        /// Which application.
        id: AppId,
    },

    /// One installed plugin. Reserved for the future plugin architecture.
    Plugin {
        /// Which plugin.
        id: PluginId,
    },
}

impl Principal {
    /// Convenience constructor for an application principal.
    #[must_use]
    pub fn app(id: AppId) -> Self {
        Self::App { id }
    }

    /// Convenience constructor for a plugin principal.
    #[must_use]
    pub fn plugin(id: PluginId) -> Self {
        Self::Plugin { id }
    }

    /// Whether this principal is Ephemeral itself.
    ///
    /// Used at enforcement points that must distinguish "the product may do
    /// this" from "this generated app may do this". Getting that distinction
    /// wrong is a privilege-escalation bug, so the check is spelled out rather
    /// than inferred.
    #[must_use]
    pub fn is_ephemeral(&self) -> bool {
        matches!(self, Self::Ephemeral)
    }

    /// The application this principal refers to, if it is an application.
    #[must_use]
    pub fn as_app(&self) -> Option<&AppId> {
        match self {
            Self::App { id } => Some(id),
            _ => None,
        }
    }

    /// A short, human-readable label for use in prompts and the audit log.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Ephemeral => "Ephemeral".to_owned(),
            Self::App { id } => format!("app:{id}"),
            Self::Plugin { id } => format!("plugin:{id}"),
        }
    }
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// Converts a human-readable name into an identifier stem.
///
/// Lowercases, replaces every run of non-alphanumeric characters with a single
/// hyphen, and trims hyphens from the ends. Non-ASCII characters are dropped
/// rather than transliterated: the result only has to be a stable, safe path
/// component, and the human-readable name is kept separately in the manifest.
fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut pending_hyphen = false;

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            pending_hyphen = true;
        }
    }

    slug
}

/// Eight hex characters of randomness — enough that ids do not collide in
/// practice and cannot be guessed, short enough to stay readable.
fn short_random_suffix() -> String {
    let uuid = uuid::Uuid::new_v4();
    uuid.simple().to_string().chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_identifiers() {
        for id in ["a", "csv-comparator", "app-1", "0", "a-b-c-1-2-3"] {
            assert!(AppId::parse(id).is_ok(), "{id} should be accepted");
        }
    }

    #[test]
    fn rejects_empty_identifiers() {
        assert_eq!(AppId::parse(""), Err(IdError::Empty));
    }

    #[test]
    fn rejects_overlong_identifiers() {
        let long = "a".repeat(MAX_ID_LENGTH + 1);
        assert_eq!(
            AppId::parse(long),
            Err(IdError::TooLong {
                length: MAX_ID_LENGTH + 1
            })
        );
        assert!(AppId::parse("a".repeat(MAX_ID_LENGTH)).is_ok());
    }

    /// The security-relevant case: an id is a path component, so anything that
    /// could escape a directory must be refused at construction.
    #[test]
    fn rejects_path_traversal_and_separators() {
        for hostile in [
            "..",
            "../etc",
            "a/b",
            "a\\b",
            "a/../b",
            "./a",
            "~",
            "a b",
            "a\0b",
            "a:b",
            "CON",
            "APP",
            "app.id",
            "app%2e%2e",
        ] {
            assert!(
                AppId::parse(hostile).is_err(),
                "{hostile:?} must be rejected: it is used as a path component"
            );
        }
    }

    #[test]
    fn rejects_malformed_hyphens() {
        for id in ["-app", "app-", "a--b"] {
            assert!(
                matches!(AppId::parse(id), Err(IdError::MalformedHyphens { .. })),
                "{id} should be rejected for hyphen placement"
            );
        }
    }

    #[test]
    fn generated_identifiers_are_valid_and_unique() {
        let a = AppId::generate("CSV Comparator");
        let b = AppId::generate("CSV Comparator");

        assert_ne!(a, b, "generated ids must not collide");
        for id in [&a, &b] {
            assert!(
                AppId::parse(id.as_str()).is_ok(),
                "generated id {id} must satisfy the same rules as a parsed one"
            );
            assert!(id.as_str().starts_with("csv-comparator-"));
        }
    }

    /// Generation must produce a valid id even for names that carry nothing
    /// usable — an empty prompt, punctuation, or a non-Latin script.
    #[test]
    fn generated_identifiers_survive_hostile_names() {
        for name in ["", "   ", "!!!", "../../etc/passwd", "日本語", "-"] {
            let id = AppId::generate(name);
            assert!(
                AppId::parse(id.as_str()).is_ok(),
                "id generated from {name:?} was invalid: {id}"
            );
        }
    }

    #[test]
    fn generated_identifiers_respect_the_length_limit() {
        let id = AppId::generate(&"very long application name ".repeat(20));
        assert!(id.as_str().len() <= MAX_ID_LENGTH);
        assert!(AppId::parse(id.as_str()).is_ok());
    }

    #[test]
    fn identifiers_round_trip_through_json() {
        let id = AppId::parse("csv-comparator").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"csv-comparator\"");
        assert_eq!(serde_json::from_str::<AppId>(&json).unwrap(), id);
    }

    /// Deserialisation must not be a way around the constructor: a manifest
    /// arriving from disk or the network gets the same validation.
    #[test]
    fn deserialisation_rejects_invalid_identifiers() {
        assert!(serde_json::from_str::<AppId>("\"../etc\"").is_err());
        assert!(serde_json::from_str::<AppId>("\"\"").is_err());
        assert!(serde_json::from_str::<AppId>("\"Not-Lowercase\"").is_err());
    }

    #[test]
    fn principals_are_distinguishable() {
        let app = Principal::app(AppId::parse("one").unwrap());
        let other = Principal::app(AppId::parse("two").unwrap());

        assert!(Principal::Ephemeral.is_ephemeral());
        assert!(!app.is_ephemeral());
        assert_ne!(app, other);
        assert_ne!(app, Principal::Ephemeral);
        assert_eq!(app.as_app().unwrap().as_str(), "one");
        assert!(Principal::Ephemeral.as_app().is_none());
    }

    #[test]
    fn slugify_collapses_separators() {
        assert_eq!(slugify("CSV Comparator"), "csv-comparator");
        assert_eq!(
            slugify("  Compare   these!!  files  "),
            "compare-these-files"
        );
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert_eq!(slugify(""), "");
    }
}
