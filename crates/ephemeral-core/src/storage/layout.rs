//! Where an application's files live.
//!
//! One predictable directory per application, with separated concerns:
//!
//! ```text
//! <data-root>/
//!   apps/<app-id>/
//!     manifest.json    what the application is
//!     source/          generated source
//!     build/           build output
//!     runtime/         runtime scratch — destroyed on teardown
//!     data/            the application's own persistent data
//!     logs/            build, test and runtime logs
//!     artifacts/       exports and reports
//!   trash/<app-id>/    deleted applications, until they are purged
//!   audit.json         the audit log
//! ```
//!
//! ## Why the layout is a security control
//!
//! The application id is the isolation unit. Nothing outside `apps/<id>/` is
//! reachable by that application, and no application's tree is ever mounted into
//! another's runtime. That gives cross-application isolation a *structural*
//! backstop, rather than resting entirely on the runtime sandbox ([ADR-0009]).
//!
//! It holds because [`AppId`] cannot contain a path separator or a `..` segment
//! — that is enforced at construction, including through deserialisation — so a
//! join can never escape. The test at the bottom of this file asserts it anyway,
//! because the consequence of being wrong is one application reading another's
//! data.
//!
//! **Secrets are not in this tree.** They live in platform-native secure storage
//! and are injected by the runtime.
//!
//! [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md

use std::path::{Path, PathBuf};

use crate::identity::AppId;

/// The file an application's manifest is stored in.
pub const MANIFEST_FILE: &str = "manifest.json";

/// The file the audit log is stored in.
pub const AUDIT_FILE: &str = "audit.json";

/// The directory holding every application.
pub const APPS_DIR: &str = "apps";

/// The directory holding deleted applications until they are purged.
pub const TRASH_DIR: &str = "trash";

/// Where Ephemeral keeps everything on this device.
///
/// Constructing a layout touches no filesystem — it only computes paths — so
/// this type is available even with the `fs` feature disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    root: PathBuf,
}

impl StorageLayout {
    /// A layout rooted at a directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where applications live.
    #[must_use]
    pub fn apps_dir(&self) -> PathBuf {
        self.root.join(APPS_DIR)
    }

    /// Where deleted applications wait to be purged.
    #[must_use]
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join(TRASH_DIR)
    }

    /// Where the audit log lives.
    #[must_use]
    pub fn audit_path(&self) -> PathBuf {
        self.root.join(AUDIT_FILE)
    }

    /// The directories belonging to one application.
    #[must_use]
    pub fn app(&self, id: &AppId) -> AppPaths {
        AppPaths {
            root: self.apps_dir().join(id.as_str()),
        }
    }

    /// Where an application's files go once it is deleted.
    #[must_use]
    pub fn trashed_app(&self, id: &AppId) -> AppPaths {
        AppPaths {
            root: self.trash_dir().join(id.as_str()),
        }
    }
}

/// The directories belonging to one application.
///
/// Every path is inside [`AppPaths::root`], which is inside the layout's
/// `apps/` directory. There is no method here that produces a path outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// The application's own directory. Everything else is inside this.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The application's manifest.
    #[must_use]
    pub fn manifest(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    /// Generated source.
    #[must_use]
    pub fn source(&self) -> PathBuf {
        self.root.join("source")
    }

    /// Every version's source, kept by digest.
    #[must_use]
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// One version's source, by its digest.
    ///
    /// [ADR-0011] says a version is immutable and identified by the digest of
    /// its content. Recording the digest without keeping the content makes that
    /// half true: the history can say what an application *was* and cannot put
    /// it back. This is where the bytes live so that it can.
    ///
    /// The digest is a hex string this crate produced, so it cannot contain a
    /// separator or a parent reference and cannot escape the application's
    /// tree — but it goes through [`AppPaths::resolve`] anyway, because a path
    /// that is safe by argument rather than by check is one refactor away from
    /// not being.
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    #[must_use]
    pub fn version_source(&self, digest: &crate::VersionDigest) -> Option<PathBuf> {
        self.resolve(&format!("versions/{}", digest.as_str()))
    }

    /// Build output.
    #[must_use]
    pub fn build(&self) -> PathBuf {
        self.root.join("build")
    }

    /// Runtime scratch. Destroyed on teardown — nothing here survives a stop.
    #[must_use]
    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// The application's own persistent data.
    #[must_use]
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// Build, test and runtime logs.
    #[must_use]
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Exports and reports the application produced.
    #[must_use]
    pub fn artifacts(&self) -> PathBuf {
        self.root.join("artifacts")
    }

    /// Every directory that should exist for a working application.
    #[must_use]
    pub fn directories(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.source(),
            self.versions(),
            self.build(),
            self.runtime(),
            self.data(),
            self.logs(),
            self.artifacts(),
        ]
    }

    /// Resolves a path an application declared, refusing anything that would
    /// leave its directory.
    ///
    /// Manifests carry relative artifact paths, and a manifest is a document a
    /// user can edit and an attacker might supply. This is the join that must
    /// not be done naively.
    ///
    /// Note that this is a *lexical* check: it cannot see through symbolic
    /// links, because this crate performs no host I/O. The runtime resolves
    /// links and applies the same rule again before mounting anything. Both
    /// checks are required; neither is sufficient alone.
    #[must_use]
    pub fn resolve(&self, relative: &str) -> Option<PathBuf> {
        if relative.trim().is_empty() || relative.contains('\0') {
            return None;
        }

        // An anchored path is refused outright rather than reinterpreted as a
        // relative one. Splitting "/etc/passwd" into segments would silently
        // turn it into "<app>/etc/passwd", which looks contained but means the
        // caller asked for something else entirely — and the next caller might
        // not split it at all.
        if relative.starts_with('/') || relative.starts_with('\\') || relative.starts_with('~') {
            return None;
        }
        if relative.len() >= 2 && relative.as_bytes()[1] == b':' {
            return None;
        }

        let mut resolved = self.root.clone();
        for segment in relative.split(['/', '\\']) {
            match segment {
                "" | "." => {}
                ".." | "~" => return None,
                other => {
                    // An absolute segment or a drive letter would replace the
                    // whole path rather than extend it, which is exactly the
                    // escape being prevented.
                    if Path::new(other).is_absolute() || other.contains(':') {
                        return None;
                    }
                    resolved.push(other);
                }
            }
        }

        // A path that resolved to nothing beyond the root is not a location
        // inside the application, it *is* the application directory.
        (resolved != self.root).then_some(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> StorageLayout {
        StorageLayout::new("/data/ephemeral")
    }

    fn id(value: &str) -> AppId {
        AppId::parse(value).unwrap()
    }

    #[test]
    fn the_layout_separates_concerns() {
        let paths = layout().app(&id("csv-comparator"));

        assert!(paths.root().ends_with("apps/csv-comparator"));
        assert!(paths.source().ends_with("csv-comparator/source"));
        assert!(paths.build().ends_with("csv-comparator/build"));
        assert!(paths.runtime().ends_with("csv-comparator/runtime"));
        assert!(paths.data().ends_with("csv-comparator/data"));
        assert!(paths.logs().ends_with("csv-comparator/logs"));
        assert!(paths.artifacts().ends_with("csv-comparator/artifacts"));
        assert!(paths.manifest().ends_with("csv-comparator/manifest.json"));
    }

    /// Every path an application has is inside its own directory. This is the
    /// structural half of cross-application isolation.
    #[test]
    fn every_application_path_stays_inside_its_own_directory() {
        let paths = layout().app(&id("csv-comparator"));

        for path in paths.directories() {
            assert!(
                path.starts_with(paths.root()),
                "{} escaped the application directory",
                path.display()
            );
        }
        assert!(paths.manifest().starts_with(paths.root()));
        assert!(paths.root().starts_with(layout().apps_dir()));
    }

    /// Two applications never share a directory, and neither contains the
    /// other — including for ids where one is a prefix of the other.
    #[test]
    fn applications_never_overlap() {
        let layout = layout();
        let a = layout.app(&id("csv"));
        let b = layout.app(&id("csv-comparator"));

        assert_ne!(a.root(), b.root());
        assert!(
            !b.root().starts_with(a.root()),
            "an id that is a prefix of another must not nest inside it"
        );
        assert!(!a.root().starts_with(b.root()));
    }

    #[test]
    fn deleted_applications_go_somewhere_separate() {
        let layout = layout();
        let live = layout.app(&id("csv-comparator"));
        let trashed = layout.trashed_app(&id("csv-comparator"));

        assert_ne!(live.root(), trashed.root());
        assert!(trashed.root().starts_with(layout.trash_dir()));
        assert!(!trashed.root().starts_with(layout.apps_dir()));
    }

    // --- resolving declared paths: security-critical ---------------------------

    #[test]
    fn declared_relative_paths_resolve_inside_the_application() {
        let paths = layout().app(&id("csv-comparator"));

        for relative in ["source", "source/app", "build/dist/index.html", "./logs"] {
            let resolved = paths
                .resolve(relative)
                .unwrap_or_else(|| panic!("{relative} should resolve"));
            assert!(
                resolved.starts_with(paths.root()),
                "{relative} resolved outside the application: {}",
                resolved.display()
            );
        }
    }

    /// The join that must not be naive. Every one of these would reach another
    /// application's data, or the host's.
    #[test]
    fn declared_paths_cannot_escape_the_application() {
        let paths = layout().app(&id("csv-comparator"));

        for hostile in [
            "..",
            "../other-app",
            "source/../../other-app/data",
            "/etc/passwd",
            "~/.ssh/id_rsa",
            "C:/Windows/System32",
            "source/../..",
            "\\\\server\\share",
            "",
            "   ",
            "sou\0rce",
            ".",
        ] {
            assert_eq!(
                paths.resolve(hostile),
                None,
                "{hostile:?} must not resolve: it would leave the application directory"
            );
        }
    }

    /// Belt and braces: even if an identifier somehow carried a separator, the
    /// join must not climb out of the apps directory. `AppId` makes this
    /// unreachable, and the test says so out loud.
    #[test]
    fn an_identifier_cannot_carry_a_path_out_of_the_apps_directory() {
        for hostile in ["../escape", "a/b", "..", "/etc"] {
            assert!(
                AppId::parse(hostile).is_err(),
                "{hostile:?} must be rejected before it can reach a path join"
            );
        }

        let layout = layout();
        for valid in ["a", "csv-comparator", "app-1"] {
            let root = layout.app(&id(valid)).root().to_path_buf();
            assert!(root.starts_with(layout.apps_dir()));
            assert!(!root.to_string_lossy().contains(".."));
        }
    }
}
