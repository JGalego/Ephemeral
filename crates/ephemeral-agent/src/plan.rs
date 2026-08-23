//! What a model proposes, in Ephemeral's own types.
//!
//! Everything here is a **proposal**. A [`Plan`] does not grant anything, a
//! [`GeneratedApp`] does not run anything, and neither can move an application
//! through its lifecycle. They are structured data that the caller validates and
//! then decides about — which is what keeps a steered model from being an
//! escalation ([ADR-0008]).
//!
//! The types are deliberately narrow. A provider cannot return "run this
//! command", "set this limit" or "grant this permission", because there is
//! nowhere in these structures to put such a thing.
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use ephemeral_core::{
    Recipe,
    manifest::{AppInterface, RuntimeKind},
    permission::AppPermission,
};

/// The largest a single generated file may be.
///
/// A bound rather than a guess: an unbounded write from a model is a disk-fill
/// waiting to happen, and nothing Ephemeral generates is a megabyte of source.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// The largest number of files one application may consist of.
pub const MAX_FILES: usize = 64;

/// What a model proposes to build, before any code exists.
///
/// The user sees this before generation starts. It is the first point at which
/// somebody can say "no, that is not what I meant" — and the first point at
/// which the permissions an application will want are visible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// What the application will do, in the user's language.
    pub summary: String,

    /// How a person will use it.
    pub interface: AppInterface,

    /// What it will run on.
    pub runtime: RuntimeKind,

    /// The base image, which the caller pins.
    pub image: String,

    /// What the application will ask to be allowed to do, and why.
    ///
    /// Requests, never grants. Each one becomes a question for a person, with
    /// the model's stated reason attached — which is why the reason travels
    /// with the request rather than being invented later.
    #[serde(default)]
    pub requests: Vec<PermissionRequest>,
}

impl Plan {
    /// Whether this plan is one Ephemeral is willing to act on.
    ///
    /// # Errors
    ///
    /// [`PlanError`] naming what is wrong, in terms a person can act on.
    pub fn validate(&self) -> Result<(), PlanError> {
        if self.summary.trim().is_empty() {
            return Err(PlanError::NoSummary);
        }
        if self.image.trim().is_empty() {
            return Err(PlanError::NoImage);
        }

        // A request with no reason cannot be put to a person as a question,
        // because "why does it want this?" would have no answer. Refusing the
        // plan is better than showing a prompt that cannot be answered.
        if let Some(unexplained) = self
            .requests
            .iter()
            .find(|request| request.reason.trim().is_empty())
        {
            return Err(PlanError::UnexplainedRequest {
                capability: unexplained.permission.capability().to_owned(),
            });
        }

        Ok(())
    }

    /// The permissions this plan asks for, without their reasons.
    #[must_use]
    pub fn requested(&self) -> Vec<AppPermission> {
        self.requests
            .iter()
            .map(|request| request.permission.clone())
            .collect()
    }
}

/// One capability an application wants, and the model's reason for wanting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    /// What is being asked for.
    pub permission: AppPermission,

    /// Why, in the user's terms.
    ///
    /// Shown in the permission prompt. A model's stated reason is an assertion
    /// rather than a justification, and the interface presents it as such — but
    /// a request with no reason at all cannot be put to a person honestly.
    pub reason: String,
}

/// A file a model wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    /// Where it goes, relative to the application's source directory.
    pub path: String,

    /// What is in it.
    pub contents: String,
}

impl SourceFile {
    /// A file.
    #[must_use]
    pub fn new(path: impl Into<String>, contents: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
        }
    }

    /// The digest of this file's contents, for the version recipe.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.contents.as_bytes());
        hex_of(&hasher.finalize())
    }

    /// Whether this path is one Ephemeral is willing to write.
    ///
    /// The check that stops a model writing outside the directory it was given.
    /// Absolute paths, traversal, Windows drive letters and backslash
    /// separators are all refused rather than normalised — a path that needs
    /// normalising to be safe is one nobody should be relying on.
    #[must_use]
    pub fn is_safe_path(&self) -> bool {
        let path = &self.path;

        !path.is_empty()
            && !path.starts_with('/')
            && !path.starts_with('\\')
            && !path.contains('\\')
            && !path.contains(':')
            && !path.contains('\0')
            && path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    }
}

/// Everything a model produced for one application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedApp {
    /// The plan it was built from.
    pub plan: Plan,

    /// The source files.
    pub files: Vec<SourceFile>,

    /// The build recipe, named explicitly so nothing has to guess.
    pub dockerfile: String,

    /// The command that starts it, already split into arguments.
    pub entrypoint: Vec<String>,

    /// What the application takes, so something can draw a form for it.
    ///
    /// Declared here rather than in the plan because the plan is written before
    /// the code is: by this point the model has actually written the argument
    /// parser, so it is describing what it built rather than what it intended.
    ///
    /// Empty is legitimate. An application that takes nothing, or one whose
    /// provider declared nothing, gets no form — which is a different thing
    /// from a form with no fields.
    #[serde(default)]
    pub inputs: Vec<ephemeral_core::manifest::Input>,

    /// The command that verifies it, already split into arguments.
    ///
    /// Required, not optional. "Ephemeral tests it" is a promise the product
    /// makes on its front page, and an application with nothing to run against
    /// it cannot pass validation — so a provider that returns none has produced
    /// something Ephemeral will not certify, rather than something that passes
    /// vacuously.
    pub test_command: Vec<String>,
}

impl GeneratedApp {
    /// Whether this is something Ephemeral is willing to write to disk.
    ///
    /// Every check here is a refusal rather than a repair. Silently fixing a
    /// path a model produced would hide the fact that it tried.
    ///
    /// # Errors
    ///
    /// [`PlanError`] naming the first problem found.
    pub fn validate(&self) -> Result<(), PlanError> {
        self.plan.validate()?;

        if self.files.is_empty() {
            return Err(PlanError::NoFiles);
        }
        if self.files.len() > MAX_FILES {
            return Err(PlanError::TooManyFiles {
                count: self.files.len(),
            });
        }
        if self.dockerfile.trim().is_empty() {
            return Err(PlanError::NoDockerfile);
        }
        if self.entrypoint.is_empty() {
            return Err(PlanError::NoEntrypoint);
        }
        if self.test_command.is_empty() {
            return Err(PlanError::NoTests);
        }

        for file in &self.files {
            if !file.is_safe_path() {
                return Err(PlanError::UnsafePath {
                    path: file.path.clone(),
                });
            }
            if file.contents.len() > MAX_FILE_BYTES {
                return Err(PlanError::FileTooLarge {
                    path: file.path.clone(),
                    bytes: file.contents.len(),
                });
            }
        }

        // Two files claiming the same path means one of them silently wins,
        // and which one would depend on iteration order.
        let mut paths: Vec<&String> = self.files.iter().map(|file| &file.path).collect();
        paths.sort();
        if let Some(duplicate) = paths.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(PlanError::DuplicatePath {
                path: duplicate[0].clone(),
            });
        }

        Ok(())
    }

    /// The recipe this application hashes to.
    ///
    /// Built from what the application *is*, so two machines that generated the
    /// same thing agree on its identity ([ADR-0011]).
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    #[must_use]
    pub fn recipe(&self, limits: &str) -> Recipe {
        let mut recipe = Recipe {
            runtime: self.plan.runtime.as_str().to_owned(),
            image: Some(self.plan.image.clone()),
            entrypoint: self.entrypoint.clone(),
            source: self
                .files
                .iter()
                .map(|file| (file.path.clone(), file.digest()))
                .chain(std::iter::once((
                    "Dockerfile".to_owned(),
                    SourceFile::new("Dockerfile", &self.dockerfile).digest(),
                )))
                .collect(),
            requests: self.plan.requested(),
            limits: limits.to_owned(),
        };
        recipe.normalise();
        recipe
    }
}

/// A model produced something Ephemeral will not act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PlanError {
    /// The plan does not say what the application would do.
    #[error("the plan does not say what the application would do")]
    NoSummary,

    /// The plan names no base image.
    #[error("the plan names no image to build on")]
    NoImage,

    /// A permission was requested with no stated reason.
    #[error(
        "the plan asks for {capability} without saying why, so there is no honest way to put \
         the question to you"
    )]
    UnexplainedRequest {
        /// Which capability.
        capability: String,
    },

    /// No source was produced.
    #[error("no source files were produced")]
    NoFiles,

    /// More files than Ephemeral will write.
    #[error("{count} files is more than an Ephemeral application should be")]
    TooManyFiles {
        /// How many were produced.
        count: usize,
    },

    /// No build recipe.
    #[error("no Dockerfile was produced, so there is nothing to build")]
    NoDockerfile,

    /// Nothing to run.
    #[error("no entry point was produced, so there would be nothing to start")]
    NoEntrypoint,

    /// Nothing to verify it with.
    #[error("no tests were produced, so there is no way to tell whether this application works")]
    NoTests,

    /// A path that would write outside the application's own directory.
    #[error("{path} is not a path inside the application, and will not be written")]
    UnsafePath {
        /// The offending path.
        path: String,
    },

    /// A file larger than Ephemeral will write.
    #[error("{path} is {bytes} bytes, which is larger than a generated file should be")]
    FileTooLarge {
        /// Which file.
        path: String,
        /// How large.
        bytes: usize,
    },

    /// Two files claim the same path.
    #[error("two files both claim the path {path}")]
    DuplicatePath {
        /// The contested path.
        path: String,
    },
}

/// What a model proposes to change after something went wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairAttempt {
    /// What the model believes went wrong, in the user's terms.
    pub diagnosis: String,

    /// The files it wants to replace, in full.
    ///
    /// Whole files rather than patches: a patch that applies cleanly to the
    /// wrong content is a silent corruption, and there is no reason to accept
    /// that risk for source this small.
    pub files: Vec<SourceFile>,
}

impl RepairAttempt {
    /// Applies this repair to a set of files, returning the result.
    ///
    /// Replaces by path and adds what is new. Never deletes: a repair that can
    /// remove files is a repair that can empty an application, and nothing
    /// about fixing a build needs that.
    #[must_use]
    pub fn applied_to(&self, existing: &[SourceFile]) -> Vec<SourceFile> {
        let mut result = existing.to_vec();

        for replacement in &self.files {
            match result.iter_mut().find(|file| file.path == replacement.path) {
                Some(existing) => existing.contents.clone_from(&replacement.contents),
                None => result.push(replacement.clone()),
            }
        }

        result
    }
}

/// Lowercase hex.
fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut out, byte| {
        // Writing to a String cannot fail, and the alternative — collecting
        // formatted Strings — allocates once per byte.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::permission::PathScope;

    fn plan() -> Plan {
        Plan {
            summary: "Compares two CSV files and shows the differences".to_owned(),
            interface: AppInterface::Web,
            runtime: RuntimeKind::Docker,
            image: "python:3.12-slim".to_owned(),
            requests: vec![PermissionRequest {
                permission: AppPermission::read(PathScope::parse("~/Downloads/**").unwrap()),
                reason: "to read the files you want compared".to_owned(),
            }],
        }
    }

    fn generated() -> GeneratedApp {
        GeneratedApp {
            plan: plan(),
            files: vec![SourceFile::new("main.py", "print('hello')\n")],
            dockerfile: "FROM python:3.12-slim\nCOPY . /app\n".to_owned(),
            entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
            test_command: vec!["python".to_owned(), "-m".to_owned(), "pytest".to_owned()],
            inputs: Vec::new(),
        }
    }

    /// The check that stops a model writing outside the directory it was given.
    #[test]
    fn a_path_that_escapes_the_application_is_refused() {
        for hostile in [
            "../../../etc/passwd",
            "/etc/passwd",
            "..",
            "src/../../out.py",
            "C:/Windows/System32/x.dll",
            "src\\main.py",
            "\\\\server\\share",
            "",
            "src//main.py",
        ] {
            assert!(
                !SourceFile::new(hostile, "x").is_safe_path(),
                "{hostile} should be refused"
            );
        }

        for fine in [
            "main.py",
            "src/main.py",
            "tests/test_main.py",
            "a/b/c/d.txt",
        ] {
            assert!(
                SourceFile::new(fine, "x").is_safe_path(),
                "{fine} should be allowed"
            );
        }
    }

    /// A hostile path is refused rather than normalised. Silently fixing it
    /// would hide the fact that it was attempted.
    #[test]
    fn a_hostile_path_fails_validation_rather_than_being_repaired() {
        let mut app = generated();
        app.files
            .push(SourceFile::new("../escape.py", "import os\n"));

        let error = app.validate().unwrap_err();
        assert!(matches!(error, PlanError::UnsafePath { .. }), "{error:?}");
    }

    /// A request nobody can explain cannot be put to a person as a question.
    #[test]
    fn a_permission_request_with_no_reason_is_refused() {
        let mut app = generated();
        app.plan.requests[0].reason = "   ".to_owned();

        let error = app.validate().unwrap_err();
        assert!(
            matches!(error, PlanError::UnexplainedRequest { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("why"), "{error}");
    }

    /// Unbounded output from a model is a disk-fill waiting to happen.
    #[test]
    fn output_is_bounded_on_both_axes() {
        let mut too_big = generated();
        too_big.files[0].contents = "x".repeat(MAX_FILE_BYTES + 1);
        assert!(matches!(
            too_big.validate().unwrap_err(),
            PlanError::FileTooLarge { .. }
        ));

        let mut too_many = generated();
        too_many.files = (0..=MAX_FILES)
            .map(|n| SourceFile::new(format!("f{n}.py"), "x"))
            .collect();
        assert!(matches!(
            too_many.validate().unwrap_err(),
            PlanError::TooManyFiles { .. }
        ));
    }

    /// Which of two files claiming one path wins would otherwise depend on
    /// iteration order.
    #[test]
    fn two_files_claiming_one_path_is_an_error() {
        let mut app = generated();
        app.files.push(SourceFile::new("main.py", "different\n"));

        assert!(matches!(
            app.validate().unwrap_err(),
            PlanError::DuplicatePath { .. }
        ));
    }

    #[test]
    fn an_application_with_nothing_to_build_or_run_is_refused() {
        let mut no_files = generated();
        no_files.files.clear();
        assert!(matches!(
            no_files.validate().unwrap_err(),
            PlanError::NoFiles
        ));

        let mut no_dockerfile = generated();
        no_dockerfile.dockerfile = "  ".to_owned();
        assert!(matches!(
            no_dockerfile.validate().unwrap_err(),
            PlanError::NoDockerfile
        ));

        let mut no_entrypoint = generated();
        no_entrypoint.entrypoint.clear();
        assert!(matches!(
            no_entrypoint.validate().unwrap_err(),
            PlanError::NoEntrypoint
        ));
    }

    /// "Ephemeral tests it" is a promise on the front page. An application with
    /// nothing to run against it must not pass validation vacuously.
    #[test]
    fn an_application_with_no_tests_cannot_be_certified() {
        let mut untested = generated();
        untested.test_command.clear();

        let error = untested.validate().unwrap_err();
        assert!(matches!(error, PlanError::NoTests), "{error:?}");
        assert!(error.to_string().contains("whether this application works"));
    }

    #[test]
    fn a_well_formed_application_validates() {
        generated().validate().unwrap();
    }

    /// The identity has to cover the build recipe too: the same source built by
    /// a different Dockerfile is a different application.
    #[test]
    fn the_recipe_covers_the_dockerfile_as_well_as_the_source() {
        let one = generated().recipe("cpu=500");

        let mut other_dockerfile = generated();
        other_dockerfile.dockerfile = "FROM alpine\n".to_owned();

        assert_ne!(one, other_dockerfile.recipe("cpu=500"));
    }

    /// A repair replaces and adds. It must not be able to empty an application.
    #[test]
    fn a_repair_replaces_and_adds_but_never_deletes() {
        let existing = vec![
            SourceFile::new("main.py", "broken\n"),
            SourceFile::new("helper.py", "fine\n"),
        ];

        let repair = RepairAttempt {
            diagnosis: "main.py had a syntax error".to_owned(),
            files: vec![
                SourceFile::new("main.py", "fixed\n"),
                SourceFile::new("new.py", "added\n"),
            ],
        };

        let result = repair.applied_to(&existing);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].contents, "fixed\n");
        assert_eq!(result[1].contents, "fine\n", "untouched files survive");
        assert!(result.iter().any(|file| file.path == "new.py"));
    }

    /// An empty repair is a no-op, not an erasure.
    #[test]
    fn an_empty_repair_changes_nothing() {
        let existing = vec![SourceFile::new("main.py", "code\n")];
        let repair = RepairAttempt {
            diagnosis: "nothing to do".to_owned(),
            files: Vec::new(),
        };

        assert_eq!(repair.applied_to(&existing), existing);
    }

    #[test]
    fn generated_applications_round_trip_through_json() {
        let app = generated();
        let json = serde_json::to_string(&app).unwrap();

        assert_eq!(serde_json::from_str::<GeneratedApp>(&json).unwrap(), app);
    }

    /// A field this version does not know about must not be silently ignored:
    /// it might be the one that matters.
    #[test]
    fn an_unknown_field_from_a_provider_is_an_error() {
        let json = r#"{"path":"main.py","contents":"x","mode":"0777"}"#;
        assert!(serde_json::from_str::<SourceFile>(json).is_err());
    }
}
