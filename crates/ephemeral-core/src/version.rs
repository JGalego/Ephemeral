//! What an application *is*, named by a digest of itself.
//!
//! An application's identity comes in two parts, and keeping them apart is the
//! whole point ([ADR-0011]):
//!
//! - [`AppId`](crate::AppId) says *which installation* — this one, on this
//!   machine, with this data and these grants.
//! - [`VersionDigest`] says *which application* — the same digest is the same
//!   application anywhere, on anybody's machine.
//!
//! The digest covers everything that determines what an application is and what
//! it may do: its runtime, its entry point, its resource ceilings, its source,
//! and the permissions it requests. It deliberately does not cover lifecycle
//! state, timestamps, grants, or anything else local to one installation —
//! those describe *this copy*, not the application.
//!
//! ## Why this exists
//!
//! Mostly for one question: **version 2 wants network access and version 1 did
//! not. Who told me?** A counter cannot answer that. A digest over the requested
//! permissions can, because two versions can be compared and the difference
//! stated before anything runs.
//!
//! [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    Timestamp,
    permission::{AppPermission, RiskLevel},
};

/// How many hex characters of a digest are shown to a person.
///
/// Enough to be unambiguous among an individual's applications, short enough to
/// read aloud. The full digest is always what is compared.
const SHORT_LENGTH: usize = 12;

/// The identity of one version of an application.
///
/// Content-addressed: derived from what the application *is*, never assigned.
/// Two installations that built the same recipe hold the same digest, and a
/// digest that differs means something that matters differed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionDigest(String);

impl VersionDigest {
    /// The digest of a recipe.
    #[must_use]
    pub fn of(recipe: &Recipe) -> Self {
        let mut hasher = Sha256::new();

        // Field-by-field with explicit separators rather than serialising the
        // struct. A serialisation format that reordered fields, or changed how
        // it renders an empty list, would silently change every digest in
        // existence — and a digest that is not stable across versions of
        // Ephemeral is not an identity.
        hasher.update(b"ephemeral-recipe-v1\n");

        hasher.update(b"runtime\n");
        hasher.update(recipe.runtime.as_bytes());
        hasher.update(b"\nimage\n");
        hasher.update(recipe.image.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"\nentrypoint\n");
        for argument in &recipe.entrypoint {
            hasher.update(argument.as_bytes());
            hasher.update(b"\x1f");
        }

        hasher.update(b"\nsource\n");
        for (path, digest) in &recipe.source {
            hasher.update(path.as_bytes());
            hasher.update(b"\x1f");
            hasher.update(digest.as_bytes());
            hasher.update(b"\x1e");
        }

        // The part this exists for. Requested permissions are inside the
        // identity, so an application cannot quietly widen what it asks for
        // while claiming to be the same version.
        hasher.update(b"\npermissions\n");
        for permission in &recipe.requests {
            hasher.update(permission.capability().as_bytes());
            hasher.update(b"\x1f");
            // The target as well as the capability: `read:~/Downloads` and
            // `read:~/.ssh` are not the same request.
            hasher.update(permission.describe().as_bytes());
            hasher.update(b"\x1e");
        }

        hasher.update(b"\nlimits\n");
        hasher.update(recipe.limits.as_bytes());

        Self(hex::encode(hasher.finalize()))
    }

    /// The full digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The abbreviated form shown to a person.
    #[must_use]
    pub fn short(&self) -> &str {
        let end = SHORT_LENGTH.min(self.0.len());
        &self.0[..end]
    }

    /// Whether this digest starts with `prefix`, for looking one up by hand.
    ///
    /// Case-insensitive, because a person retyping a digest should not be
    /// defeated by capitalisation.
    #[must_use]
    pub fn matches(&self, prefix: &str) -> bool {
        !prefix.is_empty() && self.0.starts_with(&prefix.to_ascii_lowercase())
    }
}

impl fmt::Display for VersionDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything that determines what an application is and what it may do.
///
/// Assembled from a manifest, deliberately excluding everything local to one
/// installation. If a field belongs here, changing it produces a new
/// application; if it does not, changing it does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recipe {
    /// The runtime kind, by name.
    pub runtime: String,

    /// The base image, pinned by digest wherever possible.
    pub image: Option<String>,

    /// The command that starts it, already split.
    pub entrypoint: Vec<String>,

    /// Every source file, by path and content digest, in a stable order.
    ///
    /// Content rather than paths alone, so an application whose code changed
    /// cannot claim to be the same version.
    pub source: Vec<(String, String)>,

    /// What the application asks to be allowed to do.
    pub requests: Vec<AppPermission>,

    /// The resource ceilings, rendered stably.
    pub limits: String,
}

impl Recipe {
    /// Puts the parts that must be order-independent into a stable order.
    ///
    /// Two machines that discovered the same files in a different order have
    /// the same application, so the digest must not disagree with them.
    pub fn normalise(&mut self) {
        self.source.sort();
        self.source.dedup();
        self.requests.sort();
        self.requests.dedup();
    }
}

/// One version of an application, and how it came to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// What this version is.
    pub digest: VersionDigest,

    /// The human-facing sequence number. The digest is the identity.
    pub sequence: u32,

    /// When it was produced.
    pub created_at: Timestamp,

    /// Why it exists, in the user's terms.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,

    /// What this version asks to be allowed to do.
    ///
    /// Kept on the version rather than looked up, so the delta between two
    /// versions can be computed without either of them being the current one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<AppPermission>,
}

impl Version {
    /// Records a version.
    #[must_use]
    pub fn new(recipe: &Recipe, sequence: u32, reason: impl Into<String>) -> Self {
        Self {
            digest: VersionDigest::of(recipe),
            sequence,
            created_at: crate::now(),
            reason: reason.into(),
            requests: recipe.requests.clone(),
        }
    }

    /// What moving from `self` to `next` would newly ask for.
    ///
    /// The question ADR-0011 exists to answer. Widening is a permission
    /// decision; narrowing is not.
    #[must_use]
    pub fn widening_to(&self, next: &Self) -> PermissionDelta {
        let added: Vec<AppPermission> = next
            .requests
            .iter()
            .filter(|wanted| !self.requests.iter().any(|held| held.satisfies(wanted)))
            .cloned()
            .collect();

        let removed: Vec<AppPermission> = self
            .requests
            .iter()
            .filter(|had| !next.requests.iter().any(|wanted| wanted.satisfies(had)))
            .cloned()
            .collect();

        PermissionDelta { added, removed }
    }
}

/// What changed about an application's requests between two versions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionDelta {
    /// Capabilities the new version asks for that the old one did not.
    ///
    /// Every one of these is a question for a person, not a detail of an
    /// update.
    pub added: Vec<AppPermission>,

    /// Capabilities the old version asked for and the new one does not.
    pub removed: Vec<AppPermission>,
}

impl PermissionDelta {
    /// Whether the new version wants anything the old one did not.
    #[must_use]
    pub fn widens(&self) -> bool {
        !self.added.is_empty()
    }

    /// Whether anything changed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// The highest risk among the newly requested capabilities.
    #[must_use]
    pub fn highest_added_risk(&self) -> Option<RiskLevel> {
        self.added.iter().map(AppPermission::risk).max()
    }

    /// How this is put to a person.
    ///
    /// Phrased around what the update *wants*, because that is the decision
    /// being asked for. An update that only gives things up says so, since a
    /// user seeing "this update changed its permissions" deserves to know which
    /// direction.
    #[must_use]
    pub fn describe(&self) -> String {
        match (self.added.len(), self.removed.len()) {
            (0, 0) => "This update asks for nothing new.".to_owned(),
            (0, gave_up) => {
                format!("This update gives up {gave_up} permission(s) and asks for nothing new.")
            }
            (wants, 0) => {
                format!("This update wants {wants} thing(s) the version you approved did not.")
            }
            (wants, gave_up) => format!(
                "This update wants {wants} thing(s) the version you approved did not, and \
                 gives up {gave_up}."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{HostScope, PathScope};

    fn recipe() -> Recipe {
        let mut recipe = Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
            source: vec![("main.py".to_owned(), "abc123".to_owned())],
            requests: vec![AppPermission::read(
                PathScope::parse("~/Downloads/**").unwrap(),
            )],
            limits: "cpu=500,mem=512".to_owned(),
        };
        recipe.normalise();
        recipe
    }

    /// The property the whole idea rests on: same recipe, same identity, with
    /// nothing else involved.
    #[test]
    fn the_same_recipe_has_the_same_digest() {
        assert_eq!(VersionDigest::of(&recipe()), VersionDigest::of(&recipe()));
    }

    /// Changed code is a different application, whatever it calls itself.
    #[test]
    fn changed_source_changes_the_digest() {
        let mut changed = recipe();
        changed.source = vec![("main.py".to_owned(), "def456".to_owned())];

        assert_ne!(VersionDigest::of(&recipe()), VersionDigest::of(&changed));
    }

    /// The reason this is content-addressed rather than counted. An application
    /// must not be able to ask for more while claiming to be the same version.
    #[test]
    fn asking_for_more_changes_the_digest() {
        let mut wider = recipe();
        wider
            .requests
            .push(AppPermission::outbound(HostScope::parse("*").unwrap()));
        wider.normalise();

        assert_ne!(VersionDigest::of(&recipe()), VersionDigest::of(&wider));
    }

    /// Two machines that listed the same files in a different order built the
    /// same application, and the digest has to agree with them.
    #[test]
    fn ordering_that_does_not_matter_does_not_change_the_digest() {
        let mut shuffled = recipe();
        shuffled.source = vec![
            ("b.py".to_owned(), "222".to_owned()),
            ("a.py".to_owned(), "111".to_owned()),
        ];
        shuffled.normalise();

        let mut ordered = recipe();
        ordered.source = vec![
            ("a.py".to_owned(), "111".to_owned()),
            ("b.py".to_owned(), "222".to_owned()),
        ];
        ordered.normalise();

        assert_eq!(VersionDigest::of(&shuffled), VersionDigest::of(&ordered));
    }

    /// Ordering that *does* matter must not be normalised away — an entrypoint
    /// is a command, and its arguments are not a set.
    #[test]
    fn entrypoint_order_is_part_of_the_identity() {
        let mut swapped = recipe();
        swapped.entrypoint = vec!["main.py".to_owned(), "python".to_owned()];

        assert_ne!(VersionDigest::of(&recipe()), VersionDigest::of(&swapped));
    }

    /// A digest a person can read, and look one up by.
    #[test]
    fn digests_abbreviate_and_can_be_looked_up_by_prefix() {
        let digest = VersionDigest::of(&recipe());

        assert_eq!(digest.short().len(), SHORT_LENGTH);
        assert!(digest.matches(digest.short()));
        assert!(digest.matches(&digest.short().to_ascii_uppercase()));
        assert!(!digest.matches(""), "an empty prefix must not match");
        assert!(!digest.matches("zzzzzz"));
    }

    /// The question ADR-0011 exists to answer.
    #[test]
    fn a_widening_update_is_visible_before_it_runs() {
        let first = Version::new(&recipe(), 1, "generated");

        let mut wider = recipe();
        wider
            .requests
            .push(AppPermission::outbound(HostScope::parse("*").unwrap()));
        wider.normalise();
        let second = Version::new(&wider, 2, "repaired");

        let delta = first.widening_to(&second);
        assert!(delta.widens());
        assert_eq!(delta.added.len(), 1);
        assert!(delta.removed.is_empty());
        assert!(delta.describe().contains("did not"), "{}", delta.describe());
    }

    /// An update that gives things up is not a permission decision, and must
    /// not be presented as one.
    #[test]
    fn a_narrowing_update_does_not_widen() {
        let mut wider = recipe();
        wider
            .requests
            .push(AppPermission::outbound(HostScope::parse("*").unwrap()));
        wider.normalise();

        let before = Version::new(&wider, 1, "generated");
        let after = Version::new(&recipe(), 2, "repaired");

        let delta = before.widening_to(&after);
        assert!(!delta.widens());
        assert_eq!(delta.removed.len(), 1);
        assert!(
            delta.describe().contains("nothing new"),
            "{}",
            delta.describe()
        );
    }

    /// A narrower request already covered by a wider one is not new access.
    #[test]
    fn a_request_already_covered_is_not_a_widening() {
        let mut broad = recipe();
        broad.requests = vec![AppPermission::read(
            PathScope::parse("~/Downloads/**").unwrap(),
        )];
        broad.normalise();

        let mut narrow = recipe();
        narrow.requests = vec![AppPermission::read(
            PathScope::parse("~/Downloads/apartments/**").unwrap(),
        )];
        narrow.normalise();

        let delta = Version::new(&broad, 1, "").widening_to(&Version::new(&narrow, 2, ""));
        assert!(
            !delta.widens(),
            "asking for less than was already requested is not new access"
        );
    }

    #[test]
    fn an_unchanged_update_says_so() {
        let delta = Version::new(&recipe(), 1, "").widening_to(&Version::new(&recipe(), 2, ""));

        assert!(delta.is_empty());
        assert!(!delta.widens());
        assert!(delta.describe().contains("nothing new"));
    }

    #[test]
    fn versions_round_trip_through_json() {
        let version = Version::new(&recipe(), 1, "generated from the user's request");
        let json = serde_json::to_string(&version).unwrap();

        assert_eq!(serde_json::from_str::<Version>(&json).unwrap(), version);
    }
}
