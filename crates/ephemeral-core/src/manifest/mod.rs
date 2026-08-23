//! The application manifest: what an application *is*.
//!
//! Everything else about a generated application is disposable by design. The
//! source can be regenerated, the container rebuilt, the logs discarded. The
//! manifest is the one artifact that has to survive upgrades, exports, and a
//! restore from an archive made months ago.
//!
//! It is also a **security document**: it is what a user reads to decide what an
//! app may do, so a change to its meaning changes what a past approval meant.
//! That is why the schema is versioned, why an unknown version is refused rather
//! than guessed at, and why every default is the least-privilege one
//! ([ADR-0006]).
//!
//! [ADR-0006]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0006-versioned-manifest-schema.md
//!
//! ## What is deliberately not in here
//!
//! **Secret values.** The manifest records the *names* of settings an app may
//! use; the values live in platform-native secure storage and are injected by
//! the runtime. There is no field to put a secret in, which is a stronger
//! guarantee than a rule saying not to.
//!
//! **Absolute host paths and machine identifiers.** A manifest describes an
//! application, not the machine it happens to be on, so it can move between
//! devices and platforms.
//!
//! # Example
//!
//! ```
//! use ephemeral_core::manifest::AppManifest;
//!
//! let manifest = AppManifest::from_yaml(r#"
//! schema_version: 1
//! id: apartment-comparator
//! name: Apartment Comparator
//! description: Compares two CSV files of apartment listings and shows the differences.
//! version: 1
//! runtime:
//!   type: docker
//!   image: python:3.12-slim
//!   interface: web
//!   port: 8080
//! permissions:
//!   filesystem:
//!     - read: ~/Downloads/apartments/**
//! metadata:
//!   purpose: Compare the two listing exports I downloaded.
//!   retention:
//!     policy: temporary
//!     retain_for: 7d
//! "#)?;
//!
//! assert_eq!(manifest.name, "Apartment Comparator");
//! assert_eq!(manifest.permissions.capabilities().len(), 1);
//! assert!(manifest.runtime.as_ref().is_some_and(|runtime| runtime.runs_locally()));
//! # Ok::<(), ephemeral_core::manifest::ManifestError>(())
//! ```

mod inputs;
mod metadata;
mod resources;
mod runtime;

pub use inputs::{Input, InputError, InputKind, Passing, arguments, switched_on};
pub use metadata::{Artifacts, ExecutionLocation, Metadata};

/// One capability an application asked for, and the reason it gave.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRationale {
    /// What was asked for.
    pub permission: crate::permission::AppPermission,

    /// Why, in the user's terms, as stated by whatever asked.
    pub reason: String,
}
pub use resources::{GenerationBudget, ResourceLimits};
pub use runtime::{AppInterface, RuntimeKind, RuntimeSpec};

use serde::{Deserialize, Serialize};

use crate::{
    Timestamp,
    identity::AppId,
    lifecycle::{Lifecycle, Transition, TransitionRequest},
    now,
    permission::{AppPermissions, RiskLevel},
};

/// The manifest schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The longest an application name may be.
pub const MAX_NAME_LENGTH: usize = 120;

/// The longest an application description may be.
pub const MAX_DESCRIPTION_LENGTH: usize = 2000;

/// Why a manifest was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// The manifest was not valid YAML or JSON.
    #[error("manifest could not be parsed: {message}")]
    Unparseable {
        /// What the parser said.
        message: String,
    },

    /// The manifest did not say which schema version it uses.
    ///
    /// Refused rather than assumed: guessing the version of a security document
    /// means guessing what a user consented to.
    #[error(
        "manifest has no schema_version; it must declare one (this build reads {SCHEMA_VERSION})"
    )]
    MissingSchemaVersion,

    /// The manifest declares a schema version this build cannot read.
    #[error(
        "manifest declares schema_version {found}, which this build cannot read \
         (it reads {SCHEMA_VERSION}). Newer manifests need a newer Ephemeral; older \
         ones are migrated on load."
    )]
    UnsupportedSchemaVersion {
        /// The version that was declared.
        found: u32,
    },

    /// A required field was empty or out of range.
    #[error("manifest field {field} is invalid: {problem}")]
    InvalidField {
        /// Which field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
}

impl ManifestError {
    fn invalid(field: &'static str, problem: impl Into<String>) -> Self {
        Self::InvalidField {
            field,
            problem: problem.into(),
        }
    }
}

/// Reads only the schema version, so it can be checked before anything else is
/// interpreted.
#[derive(Deserialize)]
struct SchemaProbe {
    schema_version: Option<u32>,
}

/// The complete, portable description of one generated application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    /// Which schema this document uses.
    pub schema_version: u32,

    /// The application's identity. Stable for the application's whole life.
    pub id: AppId,

    /// The name a person sees.
    pub name: String,

    /// What this application does, in the user's terms.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,

    /// The application's own version, incremented when it is regenerated or
    /// repaired.
    #[serde(default = "one")]
    pub version: u32,

    /// When it was created.
    #[serde(default = "now")]
    pub created_at: Timestamp,

    /// When it was last changed.
    #[serde(default = "now")]
    pub updated_at: Timestamp,

    /// Where it is in its life, and how it got there.
    #[serde(default)]
    pub lifecycle: Lifecycle,

    /// What it runs on, once that has been decided.
    ///
    /// `None` until planning settles it. An application that has only been
    /// requested genuinely does not know yet what kind of program it needs to
    /// be, and recording a placeholder would be a guess presented as a fact.
    /// [`AppManifest::validate`] requires it from the first build onwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeSpec>,

    /// What it is permitted to do. Empty means nothing, which is the default.
    #[serde(default)]
    pub permissions: AppPermissions,

    /// What it may consume while running.
    #[serde(default)]
    pub resources: ResourceLimits,

    /// What building it is allowed to cost.
    #[serde(default)]
    pub budget: GenerationBudget,

    /// Where its source, build output and logs live.
    #[serde(default)]
    pub artifacts: Artifacts,

    /// Tags, purpose, retention and execution location.
    #[serde(default)]
    pub metadata: Metadata,

    /// Why the application says it wants each thing it asked for.
    ///
    /// Kept because a permission prompt has to answer "why does it need this?",
    /// and the answer is produced at planning time and needed much later. It is
    /// the *application's claim*, not a justification, and the interface says so
    /// — but a request with no reason cannot be put to a person honestly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rationale: Vec<PermissionRationale>,

    /// Every version this application has been, oldest first.
    ///
    /// Kept rather than replaced, so a user can see what changed and go back to
    /// one that worked. Rolling back selects an existing version; it never
    /// mutates the current one ([ADR-0011]).
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<crate::Version>,
}

fn one() -> u32 {
    1
}

impl AppManifest {
    /// Creates a manifest for a new application.
    ///
    /// Starts with no permissions, default limits and a fresh lifecycle in
    /// [`LifecycleState::Requested`](crate::lifecycle::LifecycleState::Requested).
    /// Everything the application is allowed to do has to be added deliberately.
    #[must_use]
    pub fn new(id: AppId, name: impl Into<String>, runtime: RuntimeSpec) -> Self {
        Self {
            runtime: Some(runtime),
            ..Self::requested(id, name)
        }
    }

    /// Creates a manifest for an application that has been asked for but not yet
    /// planned.
    ///
    /// It has no runtime, no permissions and no artifacts — only an identity, a
    /// name and the intent behind it. This is what `ephemeral create` records
    /// before generation has done anything.
    #[must_use]
    pub fn requested(id: AppId, name: impl Into<String>) -> Self {
        let at = now();
        let budget = GenerationBudget::default();
        Self {
            schema_version: SCHEMA_VERSION,
            id,
            name: name.into(),
            description: String::new(),
            version: 1,
            created_at: at,
            updated_at: at,
            lifecycle: Lifecycle::with_repair_budget(budget.max_repairs),
            runtime: None,
            permissions: AppPermissions::none(),
            resources: ResourceLimits::default(),
            budget,
            artifacts: Artifacts::default(),
            metadata: Metadata::default(),
            rationale: Vec::new(),
            versions: Vec::new(),
        }
    }

    /// Sets the description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Sets the permissions.
    #[must_use]
    pub fn with_permissions(mut self, permissions: AppPermissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Checks that the manifest is internally consistent and safe to act on.
    ///
    /// Called on every load, so a hand-edited or tampered-with manifest is
    /// refused rather than partly applied.
    ///
    /// # Errors
    ///
    /// [`ManifestError::UnsupportedSchemaVersion`] or
    /// [`ManifestError::InvalidField`] describing the first problem found.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }

        let name = self.name.trim();
        if name.is_empty() {
            return Err(ManifestError::invalid(
                "name",
                "an application needs a name",
            ));
        }
        if name.len() > MAX_NAME_LENGTH {
            return Err(ManifestError::invalid(
                "name",
                format!("longer than {MAX_NAME_LENGTH} characters"),
            ));
        }
        if self.description.len() > MAX_DESCRIPTION_LENGTH {
            return Err(ManifestError::invalid(
                "description",
                format!("longer than {MAX_DESCRIPTION_LENGTH} characters"),
            ));
        }
        if self.version == 0 {
            return Err(ManifestError::invalid("version", "versions start at 1"));
        }

        if !self.resources.is_valid() {
            return Err(ManifestError::invalid(
                "resources",
                "every limit must be greater than zero; omit a field to mean unlimited",
            ));
        }
        if !self.budget.is_valid() {
            return Err(ManifestError::invalid(
                "budget",
                "the generation budget must have a positive duration",
            ));
        }

        match &self.runtime {
            None if self.lifecycle.state().requires_runtime() => {
                return Err(ManifestError::invalid(
                    "runtime",
                    format!(
                        "an application that is {} must know what it runs on",
                        self.lifecycle.state()
                    ),
                ));
            }
            None => {}
            Some(runtime) => {
                // A containerised runtime without an image has nothing to run,
                // and a web app with no port cannot be opened. Both would fail
                // later, less legibly.
                //
                // Asked of a state that could actually be started, and not of
                // every state. An application between being described and being
                // built knows what runtime it wants and has nothing built yet,
                // and so does one that has just been rolled back: `revert_to`
                // clears the image deliberately, because running the newer
                // build under an older version's name would report one thing
                // and run another. Requiring an image there made a rollback
                // produce a manifest that could not be saved — the refusal
                // arrived at the end, after the source on disk had already gone
                // back, which is the half-done state the operation is written
                // to avoid.
                if runtime.kind.runs_an_image()
                    && runtime.image.is_none()
                    && self.lifecycle.state().is_runnable()
                {
                    return Err(ManifestError::invalid(
                        "runtime.image",
                        format!(
                            "an application that is {} has to have something to run",
                            self.lifecycle.state()
                        ),
                    ));
                }
                // The same requirement for the runtime that has a module rather
                // than an image. An application that can be started and does not
                // say what to start is the same hole either way.
                if runtime.kind == RuntimeKind::Wasm
                    && runtime.program.is_none()
                    && self.lifecycle.state().is_runnable()
                {
                    return Err(ManifestError::invalid(
                        "runtime.program",
                        format!(
                            "an application that is {} has to say which file to run",
                            self.lifecycle.state()
                        ),
                    ));
                }
                // Only where a web application is something that listens. On
                // WebAssembly it is something that writes a page, which needs
                // no port — and no network permission.
                if runtime.interface == AppInterface::Web
                    && runtime.kind.serves_over_a_port()
                    && runtime.port.is_none()
                {
                    return Err(ManifestError::invalid(
                        "runtime.port",
                        "a web application needs a port to be reachable on",
                    ));
                }
            }
        }

        self.artifacts.validate()?;

        Ok(())
    }

    /// Parses a manifest from YAML.
    ///
    /// The schema version is checked *before* anything else is interpreted, so
    /// a document written for a different schema is refused whole rather than
    /// partly understood.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] describing why the manifest was rejected.
    pub fn from_yaml(yaml: &str) -> Result<Self, ManifestError> {
        let probe: SchemaProbe = serde_norway::from_str(yaml).map_err(unparseable)?;
        check_version(probe.schema_version)?;

        let manifest: Self = serde_norway::from_str(yaml).map_err(unparseable)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parses a manifest from JSON.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] describing why the manifest was rejected.
    pub fn from_json(json: &str) -> Result<Self, ManifestError> {
        let probe: SchemaProbe = serde_json::from_str(json).map_err(unparseable)?;
        check_version(probe.schema_version)?;

        let manifest: Self = serde_json::from_str(json).map_err(unparseable)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Renders the manifest as YAML — the form a person reads.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Unparseable`] if the manifest cannot be serialised,
    /// which would indicate a bug rather than bad input.
    pub fn to_yaml(&self) -> Result<String, ManifestError> {
        serde_norway::to_string(self).map_err(unparseable)
    }

    /// Renders the manifest as JSON — the form the API and storage exchange.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Unparseable`] if the manifest cannot be serialised.
    pub fn to_json(&self) -> Result<String, ManifestError> {
        serde_json::to_string_pretty(self).map_err(unparseable)
    }

    /// The riskiest thing this application asks to do, if it asks for anything.
    ///
    /// What an interface uses to decide how emphatically to present the
    /// permission summary.
    #[must_use]
    pub fn highest_risk(&self) -> Option<RiskLevel> {
        self.permissions.highest_risk()
    }

    /// A one-line summary for a list view.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} — {} ({})",
            self.name,
            self.lifecycle.state().headline(),
            self.metadata.retention.headline().to_lowercase()
        )
    }

    /// Applies a lifecycle transition, refusing any that would leave the
    /// manifest invalid.
    ///
    /// Some transitions are legal for the state machine but not for this
    /// particular application. Restoring from the archive moves an application
    /// to `Ready`, which requires a runtime — and an application that was
    /// cancelled during planning never got one. The state machine cannot know
    /// that; the manifest can.
    ///
    /// On refusal the lifecycle is left exactly as it was, so a rejected
    /// transition never leaves a half-applied record behind.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Lifecycle`] if the transition is not permitted, or
    /// [`crate::Error::Manifest`] if applying it would produce an invalid
    /// manifest.
    pub fn apply(&mut self, request: TransitionRequest) -> Result<Transition, crate::Error> {
        let before = self.lifecycle.clone();
        let transition = self.lifecycle.apply(request)?.clone();

        if let Err(error) = self.validate() {
            self.lifecycle = before;
            return Err(error.into());
        }

        self.touch();
        Ok(transition)
    }

    /// Records that the application changed.
    pub fn touch(&mut self) {
        self.updated_at = now();
    }

    /// What the application said when it asked for `permission`.
    ///
    /// `None` when nothing was recorded, which the interface must present as
    /// "it gave no reason" rather than inventing one.
    #[must_use]
    pub fn reason_for(&self, permission: &crate::permission::AppPermission) -> Option<&str> {
        self.rationale
            .iter()
            .find(|entry| entry.permission == *permission)
            .map(|entry| entry.reason.as_str())
    }

    /// What this application currently is, by digest.
    ///
    /// `None` before anything has been generated. An application that has only
    /// been requested has no identity beyond its id, because there is nothing
    /// yet to take a digest of.
    #[must_use]
    pub fn current_version(&self) -> Option<&crate::Version> {
        self.versions.last()
    }

    /// Returns the application to a version it used to be.
    ///
    /// The history is append-only, so this does not truncate it: returning to
    /// an earlier version appends a new entry carrying that version's digest
    /// and its requests. Two entries with one digest is not a contradiction —
    /// the digest is the content, and the content really was current twice.
    ///
    /// Returns what the older version asks for that the current one does not.
    /// That is usually empty, and when it is not it is a permission decision
    /// exactly as an update is: rolling *back* can widen, if the version being
    /// left behind had dropped a capability, and it must be put to a person
    /// rather than applied ([ADR-0011]).
    ///
    /// **The built image is cleared.** A version is its source, and the image
    /// built from the newer source is still named in the manifest at this
    /// point. Leaving it would mean running the newer code under the older
    /// version's name — the application would report one identity and execute
    /// another, which is worse than not being able to roll back at all. The
    /// caller must build again before running.
    ///
    /// # Errors
    ///
    /// If no recorded version has that digest, or if it is already current.
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    pub fn revert_to(
        &mut self,
        digest: &crate::VersionDigest,
    ) -> Result<crate::PermissionDelta, ManifestError> {
        if self
            .current_version()
            .is_some_and(|it| it.digest == *digest)
        {
            return Err(ManifestError::invalid(
                "version",
                "that version is already the current one",
            ));
        }

        let target = self
            .versions
            .iter()
            .find(|version| version.digest == *digest)
            .ok_or_else(|| ManifestError::invalid("version", "no such version was ever recorded"))?
            .clone();

        let sequence = u32::try_from(self.versions.len().saturating_add(1)).unwrap_or(u32::MAX);
        let next = crate::Version {
            digest: target.digest.clone(),
            sequence,
            created_at: crate::now(),
            reason: format!("returned to version {}", target.sequence),
            requests: target.requests.clone(),
        };

        let delta = self
            .current_version()
            .map(|current| current.widening_to(&next))
            .unwrap_or_default();

        if let Some(spec) = self.runtime.as_mut() {
            spec.image = None;
        }

        self.version = next.sequence;
        self.versions.push(next);
        self.touch();

        Ok(delta)
    }

    /// Records a new version, produced from `recipe`.
    ///
    /// Returns what this update would newly ask for, compared with the version
    /// before it. That delta is a permission decision when it is non-empty, and
    /// the caller is expected to put it to a person rather than apply it
    /// ([ADR-0011]).
    ///
    /// A recipe identical to the current version is not recorded again: the
    /// same application is the same version, however many times it is produced.
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    pub fn record_version(
        &mut self,
        recipe: &crate::Recipe,
        reason: impl Into<String>,
    ) -> crate::PermissionDelta {
        let digest = crate::VersionDigest::of(recipe);

        if let Some(current) = self.current_version()
            && current.digest == digest
        {
            return crate::PermissionDelta::default();
        }

        // The Nth recorded version is sequence N. Deriving it from the history
        // rather than from `self.version` keeps the two from drifting apart —
        // an application created but never generated sits at version 1 with no
        // versions recorded, and its first build should be version 1, not 2.
        let sequence = u32::try_from(self.versions.len().saturating_add(1)).unwrap_or(u32::MAX);
        let next = crate::Version::new(recipe, sequence, reason);
        let delta = self
            .current_version()
            .map(|current| current.widening_to(&next));

        self.version = next.sequence;
        self.versions.push(next);
        self.touch();

        delta.unwrap_or_default()
    }
}

fn check_version(declared: Option<u32>) -> Result<(), ManifestError> {
    match declared {
        None => Err(ManifestError::MissingSchemaVersion),
        Some(SCHEMA_VERSION) => Ok(()),
        // When schema 2 exists, this is where a migration is dispatched. Until
        // then, refusing is the honest answer: there is nothing to migrate from
        // and nothing to guess at.
        Some(found) => Err(ManifestError::UnsupportedSchemaVersion { found }),
    }
}

fn unparseable(error: impl std::fmt::Display) -> ManifestError {
    ManifestError::Unparseable {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {

    /// A version history is what makes rollback and comparison possible, so
    /// recording one has to actually keep the old ones.
    #[test]
    fn recording_a_version_keeps_the_previous_ones() {
        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");
        assert_eq!(manifest.current_version(), None, "nothing generated yet");

        let mut first = crate::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
            source: vec![("main.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        first.normalise();

        manifest.record_version(&first, "generated");
        assert_eq!(manifest.versions.len(), 1);

        let mut second = first.clone();
        second.source = vec![("main.py".to_owned(), "bbb".to_owned())];
        second.normalise();

        manifest.record_version(&second, "repaired");
        assert_eq!(manifest.versions.len(), 2, "the old version survives");
        assert_ne!(manifest.versions[0].digest, manifest.versions[1].digest);
        assert_eq!(manifest.version, 2);
    }

    fn recipe(contents: &str) -> crate::Recipe {
        let mut recipe = crate::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
            source: vec![("main.py".to_owned(), contents.to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        recipe.normalise();
        recipe
    }

    fn generated_twice() -> AppManifest {
        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");
        manifest.record_version(&recipe("aaa"), "generated");
        manifest.record_version(&recipe("bbb"), "repaired");
        manifest
    }

    /// Rolling back appends rather than truncating: the history is what
    /// happened, and an application really was each of those things in turn.
    #[test]
    fn returning_to_an_earlier_version_appends_it_rather_than_erasing_the_newer_one() {
        let mut manifest = generated_twice();
        let first = manifest.versions[0].digest.clone();

        manifest.revert_to(&first).expect("the first version");

        assert_eq!(manifest.versions.len(), 3, "the newer version must survive");
        assert_eq!(manifest.current_version().unwrap().digest, first);
        assert_eq!(manifest.version, 3);
        assert_eq!(
            manifest.versions[1].digest,
            crate::VersionDigest::of(&recipe("bbb")),
            "the version rolled away from is still in the history"
        );
        assert_eq!(
            manifest.versions[2].digest, first,
            "the appended entry carries the digest of the version returned to"
        );
    }

    /// A version is its source. The image built from the *newer* source is
    /// still named in the manifest at the moment of rollback, and leaving it
    /// there would run the newer code under the older version's name — the
    /// application would report one identity and execute another, which is
    /// worse than not being able to roll back at all.
    #[test]
    fn rolling_back_clears_the_image_built_from_the_newer_source() {
        let mut manifest = generated_twice();
        manifest.runtime = Some(RuntimeSpec {
            image: Some("ephemeral/csv-comparator:newer".to_owned()),
            ..RuntimeSpec::docker_job("python:3.12-slim", vec!["python".to_owned()])
        });

        let first = manifest.versions[0].digest.clone();
        manifest.revert_to(&first).expect("the first version");

        assert_eq!(
            manifest.runtime.as_ref().unwrap().image,
            None,
            "the newer build survived a rollback and would have been run"
        );
    }

    /// Rolling back can widen. If the version being left behind had dropped a
    /// capability, returning to the older one asks for it again — and that is
    /// a question for a person, exactly as an update is.
    #[test]
    fn returning_to_a_version_that_wanted_more_is_reported_as_widening() {
        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");

        let mut hungry = recipe("aaa");
        hungry.requests = vec![crate::permission::AppPermission::read(
            crate::permission::PathScope::parse("~/Downloads/**").unwrap(),
        )];
        hungry.normalise();
        manifest.record_version(&hungry, "generated");

        let mut modest = recipe("bbb");
        modest.normalise();
        manifest.record_version(&modest, "repaired, and it stopped needing the disk");

        let back = manifest.versions[0].digest.clone();
        let delta = manifest.revert_to(&back).expect("the first version");

        assert!(
            delta.widens(),
            "returning to a version that reads the disk must be a permission decision"
        );
        assert_eq!(delta.added.len(), 1);
    }

    /// Rolling back to where you already are is a mistake worth naming rather
    /// than a no-op that quietly appends a duplicate.
    #[test]
    fn returning_to_the_current_version_is_refused() {
        let mut manifest = generated_twice();
        let current = manifest.current_version().unwrap().digest.clone();

        assert!(manifest.revert_to(&current).is_err());
        assert_eq!(manifest.versions.len(), 2, "nothing may have been appended");
    }

    /// A digest that was never recorded is not a version of this application,
    /// whatever it is a digest of.
    #[test]
    fn returning_to_a_version_that_never_existed_is_refused() {
        let mut manifest = generated_twice();
        let stranger = crate::VersionDigest::of(&recipe("never generated"));

        assert!(manifest.revert_to(&stranger).is_err());
        assert_eq!(manifest.versions.len(), 2);
    }

    /// The same application produced twice is the same version. Recording it
    /// again would make the history claim a change that did not happen.
    #[test]
    fn regenerating_the_same_application_records_no_new_version() {
        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");

        let mut recipe = crate::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned()],
            source: vec![("main.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        recipe.normalise();

        manifest.record_version(&recipe, "generated");
        let after_first = manifest.version;

        let delta = manifest.record_version(&recipe, "generated again");

        assert_eq!(manifest.versions.len(), 1);
        assert_eq!(manifest.version, after_first);
        assert!(delta.is_empty());
    }

    /// The question ADR-0011 exists to answer, reachable from the manifest.
    #[test]
    fn a_version_that_wants_more_reports_the_widening() {
        use crate::permission::{AppPermission, HostScope};

        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");

        let mut modest = crate::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned()],
            source: vec![("main.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        modest.normalise();
        manifest.record_version(&modest, "generated");

        let mut greedy = modest.clone();
        greedy.requests = vec![AppPermission::outbound(HostScope::parse("*").unwrap())];
        greedy.normalise();

        let delta = manifest.record_version(&greedy, "regenerated");

        assert!(delta.widens(), "a new network request must be visible");
        assert_eq!(delta.added.len(), 1);
    }

    #[test]
    fn a_manifest_with_versions_round_trips_through_yaml() {
        let mut manifest =
            AppManifest::requested(AppId::parse("csv-comparator").unwrap(), "CSV comparator");
        let mut recipe = crate::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned()],
            source: vec![("main.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        recipe.normalise();
        manifest.record_version(&recipe, "generated");

        let yaml = serde_norway::to_string(&manifest).unwrap();
        let parsed: AppManifest = serde_norway::from_str(&yaml).unwrap();

        assert_eq!(parsed.versions, manifest.versions);
    }
    use super::*;
    use crate::permission::{FilesystemRule, PathScope};
    use crate::retention::RetentionPolicy;

    fn id(value: &str) -> AppId {
        AppId::parse(value).unwrap()
    }

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    /// The canonical example application from the product brief.
    fn apartment_comparator() -> AppManifest {
        AppManifest::new(
            id("apartment-comparator"),
            "Apartment Comparator",
            RuntimeSpec::docker_web("python:3.12-slim", 8080),
        )
        .with_description("Compares two CSV files of apartment listings.")
        .with_permissions(AppPermissions {
            filesystem: vec![FilesystemRule::Read(scope("~/Downloads/apartments/**"))],
            ..AppPermissions::none()
        })
        .with_metadata(Metadata {
            purpose: "Compare the two listing exports I downloaded.".to_owned(),
            retention: RetentionPolicy::default(),
            ..Metadata::default()
        })
    }

    /// Drives a manifest to `Ready`, the way generation does.
    ///
    /// Written out rather than assigned, because the state a manifest is in is
    /// reached through the machine and not set — a test that set it could
    /// assert something no application can actually be.
    fn built(mut manifest: AppManifest) -> AppManifest {
        use crate::actor::Actor;
        use crate::lifecycle::LifecycleEvent;

        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
            (LifecycleEvent::ValidationPassed, Actor::Ephemeral),
        ] {
            manifest
                .apply(crate::lifecycle::TransitionRequest::new(
                    event,
                    actor,
                    "reaching ready",
                ))
                .expect("the route to ready");
        }

        manifest
    }

    // --- least privilege by default -------------------------------------------

    /// A new application asks for nothing. Every capability it ends up with was
    /// added deliberately and approved by a person.
    #[test]
    fn a_new_manifest_grants_nothing_and_is_bounded() {
        let manifest = AppManifest::new(
            id("fresh"),
            "Fresh",
            RuntimeSpec::docker_job("alpine", vec!["true".to_owned()]),
        );

        assert!(manifest.permissions.is_empty());
        assert_eq!(manifest.highest_risk(), None);
        assert!(manifest.resources.is_valid());
        assert!(manifest.budget.is_valid());
        assert_eq!(
            manifest.lifecycle.state(),
            crate::lifecycle::LifecycleState::Requested
        );
        manifest.validate().unwrap();
    }

    /// The repair budget the manifest declares is the one the state machine
    /// enforces. Two numbers that could disagree would mean the visible limit
    /// is not the real one.
    #[test]
    fn the_declared_budget_is_the_enforced_budget() {
        let manifest = AppManifest::new(
            id("fresh"),
            "Fresh",
            RuntimeSpec::docker_job("alpine", vec!["true".to_owned()]),
        );
        assert_eq!(
            manifest.lifecycle.repair_budget(),
            manifest.budget.max_repairs
        );
    }

    /// A manifest that omits its permission block must grant nothing, not
    /// inherit something.
    #[test]
    fn a_manifest_without_a_permission_block_grants_nothing() {
        let manifest = AppManifest::from_yaml(
            "schema_version: 1\n\
             id: minimal\n\
             name: Minimal\n\
             runtime:\n  type: docker\n  image: alpine\n  interface: job\n",
        )
        .unwrap();

        assert!(manifest.permissions.is_empty());
        assert_eq!(manifest.metadata.retention, RetentionPolicy::default());
        assert!(manifest.resources.is_valid());
    }

    // --- schema versioning -----------------------------------------------------

    /// Refusing beats guessing: a manifest without a version could mean anything.
    #[test]
    fn a_manifest_without_a_schema_version_is_refused() {
        let error = AppManifest::from_yaml(
            "id: minimal\nname: Minimal\nruntime:\n  type: docker\n  image: alpine\n  interface: job\n",
        )
        .unwrap_err();
        assert_eq!(error, ManifestError::MissingSchemaVersion);
    }

    #[test]
    fn an_unknown_schema_version_is_refused_whole() {
        for version in [0, 2, 99] {
            let error = AppManifest::from_yaml(&format!(
                "schema_version: {version}\n\
                 id: minimal\nname: Minimal\n\
                 runtime:\n  type: docker\n  image: alpine\n  interface: job\n"
            ))
            .unwrap_err();
            assert_eq!(
                error,
                ManifestError::UnsupportedSchemaVersion { found: version }
            );
        }
    }

    /// The version is checked before anything else is interpreted, so a
    /// document written against a different schema cannot be partly applied.
    #[test]
    fn the_version_is_checked_before_the_rest_of_the_document() {
        let error = AppManifest::from_yaml(
            "schema_version: 2\npermissions:\n  process:\n    execute: true\n",
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ManifestError::UnsupportedSchemaVersion { found: 2 }
        ));
    }

    // --- validation ------------------------------------------------------------

    /// The invariant that matters, at the moment it matters: an application
    /// cannot *become* ready with nothing to run. Relaxing the stored-manifest
    /// rule to runnable states only would be a hole if this were not true, so
    /// it is asserted rather than reasoned about.
    #[test]
    fn an_application_cannot_become_ready_with_nothing_to_run() {
        use crate::actor::Actor;
        use crate::lifecycle::{LifecycleEvent, TransitionRequest};

        let spec = apartment_comparator().runtime.clone().unwrap();
        let mut manifest = AppManifest {
            runtime: Some(RuntimeSpec {
                image: None,
                ..spec
            }),
            ..apartment_comparator()
        };

        for (event, actor) in [
            (LifecycleEvent::Plan, Actor::Ephemeral),
            (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
            (LifecycleEvent::GenerationCompleted, Actor::Agent),
            (LifecycleEvent::BuildSucceeded, Actor::Runtime),
        ] {
            manifest
                .apply(TransitionRequest::new(event, actor, "on the way"))
                .expect("everything before ready is fine without a build");
        }

        let refused = manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::ValidationPassed,
                Actor::Ephemeral,
                "claiming to be ready",
            ))
            .expect_err("ready with nothing to run");

        assert!(
            matches!(
                refused,
                crate::Error::Manifest(ManifestError::InvalidField {
                    field: "runtime.image",
                    ..
                })
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn validation_rejects_manifests_that_could_not_work() {
        let base = apartment_comparator();

        let unnamed = AppManifest {
            name: "   ".to_owned(),
            ..base.clone()
        };
        assert!(matches!(
            unnamed.validate(),
            Err(ManifestError::InvalidField { field: "name", .. })
        ));

        let spec = base.runtime.clone().unwrap();
        // Asked of a state that could be started. A stored manifest claiming to
        // be ready with nothing to run is a contradiction, and one that has not
        // been built yet is an ordinary Tuesday.
        let imageless = AppManifest {
            runtime: Some(RuntimeSpec {
                image: None,
                ..spec.clone()
            }),
            ..built(base.clone())
        };
        assert!(matches!(
            imageless.validate(),
            Err(ManifestError::InvalidField {
                field: "runtime.image",
                ..
            })
        ));
        assert!(
            AppManifest {
                runtime: Some(RuntimeSpec {
                    image: None,
                    ..spec.clone()
                }),
                ..base.clone()
            }
            .validate()
            .is_ok(),
            "an application that has not been built yet has nothing to run, and that is normal"
        );

        let unreachable = AppManifest {
            runtime: Some(RuntimeSpec { port: None, ..spec }),
            ..base.clone()
        };
        assert!(matches!(
            unreachable.validate(),
            Err(ManifestError::InvalidField {
                field: "runtime.port",
                ..
            })
        ));

        let unlimited = AppManifest {
            resources: ResourceLimits {
                memory_mib: 0,
                ..ResourceLimits::default()
            },
            ..base
        };
        assert!(matches!(
            unlimited.validate(),
            Err(ManifestError::InvalidField {
                field: "resources",
                ..
            })
        ));
    }

    /// A typo in a manifest must be an error, not a silently ignored key that
    /// leaves the user believing they restricted something.
    #[test]
    fn unknown_manifest_keys_are_refused() {
        let error = AppManifest::from_yaml(
            "schema_version: 1\nid: minimal\nname: Minimal\nnetwork_access: true\n\
             runtime:\n  type: docker\n  image: alpine\n  interface: job\n",
        )
        .unwrap_err();
        assert!(matches!(error, ManifestError::Unparseable { .. }));
    }

    /// An identifier that could escape its storage directory must be refused at
    /// the manifest boundary, not discovered later at a path join.
    #[test]
    fn a_hostile_identifier_is_refused_at_load() {
        let error = AppManifest::from_yaml(
            "schema_version: 1\nid: ../../etc\nname: Escape\n\
             runtime:\n  type: docker\n  image: alpine\n  interface: job\n",
        )
        .unwrap_err();
        assert!(matches!(error, ManifestError::Unparseable { .. }));
    }

    /// Likewise a permission scope: deserialisation runs the same parser as
    /// construction, so a hand-edited manifest cannot smuggle in a traversal.
    #[test]
    fn a_hostile_permission_scope_is_refused_at_load() {
        let error = AppManifest::from_yaml(
            "schema_version: 1\nid: escape\nname: Escape\n\
             runtime:\n  type: docker\n  image: alpine\n  interface: job\n\
             permissions:\n  filesystem:\n    - read: ~/Downloads/../../etc/shadow\n",
        )
        .unwrap_err();
        assert!(matches!(error, ManifestError::Unparseable { .. }));
    }

    // --- round trips -----------------------------------------------------------

    /// An application that has only been requested has no runtime yet, and that
    /// is a valid manifest — planning is what decides.
    #[test]
    fn a_requested_application_needs_no_runtime() {
        let requested = AppManifest::requested(id("csv-comparator"), "CSV Comparator");

        assert!(requested.runtime.is_none());
        assert_eq!(
            requested.lifecycle.state(),
            crate::lifecycle::LifecycleState::Requested
        );
        requested.validate().unwrap();

        let yaml = requested.to_yaml().unwrap();
        assert!(
            !yaml.lines().any(|line| line.starts_with("runtime:")),
            "an absent runtime is omitted entirely:\n{yaml}"
        );
        assert_eq!(AppManifest::from_yaml(&yaml).unwrap(), requested);
    }

    /// From the first build onwards it must know what it runs on, or everything
    /// downstream is acting on a guess.
    #[test]
    fn an_application_that_can_build_must_have_a_runtime() {
        use crate::actor::Actor;
        use crate::lifecycle::{LifecycleEvent, TransitionRequest};

        let mut manifest = AppManifest::requested(id("csv-comparator"), "CSV Comparator");
        for event in [
            LifecycleEvent::Plan,
            LifecycleEvent::PlanCompleted,
            LifecycleEvent::GenerationCompleted,
        ] {
            manifest
                .lifecycle
                .apply(TransitionRequest::new(event, Actor::Ephemeral, "working"))
                .unwrap();
        }

        assert!(manifest.lifecycle.state().requires_runtime());
        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::InvalidField {
                field: "runtime",
                ..
            })
        ));

        manifest.runtime = Some(RuntimeSpec::docker_job("alpine", vec!["true".to_owned()]));
        manifest.validate().unwrap();
    }

    #[test]
    fn manifests_round_trip_through_yaml_and_json() {
        let manifest = apartment_comparator();

        let yaml = manifest.to_yaml().unwrap();
        assert_eq!(AppManifest::from_yaml(&yaml).unwrap(), manifest);

        let json = manifest.to_json().unwrap();
        assert_eq!(AppManifest::from_json(&json).unwrap(), manifest);
    }

    #[test]
    fn the_yaml_form_is_readable() {
        let yaml = apartment_comparator().to_yaml().unwrap();

        for expected in [
            "schema_version: 1",
            "id: apartment-comparator",
            "name: Apartment Comparator",
            "- read: ~/Downloads/apartments/**",
            "type: docker",
        ] {
            assert!(yaml.contains(expected), "expected {expected:?} in:\n{yaml}");
        }
    }

    /// There is nowhere in a manifest to put a secret value. This is
    /// structural: the permission model records the *names* of settings, and
    /// the runtime supplies the values from secure storage.
    #[test]
    fn a_manifest_has_nowhere_to_put_a_secret_value() {
        let manifest = AppManifest {
            permissions: AppPermissions {
                environment: vec!["API_KEY".to_owned()],
                ..AppPermissions::none()
            },
            ..apartment_comparator()
        };

        let yaml = manifest.to_yaml().unwrap();
        assert!(yaml.contains("API_KEY"), "the name is recorded");

        // Deserialising a manifest that tries to supply a value must fail: the
        // environment list is names, and there is no field for values.
        assert!(
            serde_norway::from_str::<AppPermissions>(
                "environment:\n  - name: API_KEY\n    value: sk-secret\n"
            )
            .is_err()
        );
    }

    /// A transition the state machine allows but this application cannot
    /// satisfy is refused, and refusing it changes nothing.
    #[test]
    fn a_transition_that_would_invalidate_the_manifest_is_refused() {
        use crate::actor::Actor;
        use crate::lifecycle::{LifecycleEvent, LifecycleState};

        // Cancelled during planning, so it never acquired a runtime, then put
        // away. Restoring would move it to Ready, which requires one.
        let mut manifest = AppManifest::requested(id("csv-comparator"), "CSV Comparator");
        for event in [LifecycleEvent::Cancel, LifecycleEvent::Archive] {
            manifest
                .apply(TransitionRequest::new(
                    event,
                    Actor::User,
                    "changed my mind",
                ))
                .unwrap();
        }
        assert_eq!(manifest.lifecycle.state(), LifecycleState::Archived);

        let error = manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Restore,
                Actor::User,
                "actually I do want it",
            ))
            .unwrap_err();

        assert!(matches!(error, crate::Error::Manifest(_)));
        assert_eq!(
            manifest.lifecycle.state(),
            LifecycleState::Archived,
            "a refused transition must leave the application exactly where it was"
        );
        manifest.validate().unwrap();
    }

    #[test]
    fn a_transition_that_keeps_the_manifest_valid_is_applied() {
        use crate::actor::Actor;
        use crate::lifecycle::{LifecycleEvent, LifecycleState};

        let mut manifest = apartment_comparator();
        let transition = manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "working out what this needs",
            ))
            .unwrap();

        assert_eq!(transition.to, LifecycleState::Planning);
        assert_eq!(manifest.lifecycle.state(), LifecycleState::Planning);
    }

    #[test]
    fn manifests_summarise_themselves() {
        let summary = apartment_comparator().summary();
        assert!(summary.contains("Apartment Comparator"));
        assert!(summary.contains("Requested"));
    }
}
