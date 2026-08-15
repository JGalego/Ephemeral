//! Everything Ephemeral keeps on one device, in one place.
//!
//! An application on its own is not enough to act on. To answer "may this app
//! read that file?" you need the manifest *and* the permission ledger, and to
//! answer "what happened here?" you need the audit log as well. A
//! [`Workspace`] is those three things loaded together from one directory.
//!
//! It is deliberately **persistence only** — load, hold, save. It does not
//! decide anything. The service layer that orchestrates generation, runtimes and
//! permission prompts is a separate concern
//! ([`ARCHITECTURE.md` §5](https://github.com/JGalego/Ephemeral/blob/main/ARCHITECTURE.md));
//! this is what that layer will sit on, and what the CLI sits on today.
//!
//! Keeping it here rather than in a client means the desktop application and the
//! CLI cannot end up with two subtly different ideas of what is on disk.

use std::path::Path;

use super::{AUDIT_FILE, AppStore, FileStore, StorageError, StorageLayout, file::write_atomically};
use crate::{
    audit::{AuditLog, Redactor},
    identity::AppId,
    manifest::AppManifest,
    permission::PermissionLedger,
};

/// The file the permission ledger is stored in.
pub const LEDGER_FILE: &str = "permissions.json";

/// The result of reading every application in a workspace.
///
/// Split rather than all-or-nothing on purpose: one unreadable manifest must not
/// stop the product listing everything else, but it must still be reported
/// rather than quietly skipped.
#[derive(Debug, Default)]
pub struct LoadedApps {
    /// The applications that could be read.
    pub loaded: Vec<AppManifest>,

    /// The ones that could not, each with the reason.
    pub broken: Vec<(AppId, String)>,
}

/// One device's Ephemeral state: its applications, its permissions and its
/// audit record.
#[derive(Debug)]
pub struct Workspace {
    apps: FileStore,
    ledger: PermissionLedger,
    audit: AuditLog,
}

impl Workspace {
    /// Opens the workspace rooted at a directory, creating nothing.
    ///
    /// A directory that does not exist yet is not an error — it is a device
    /// where Ephemeral has not been used. It opens empty, and the first save
    /// creates it.
    ///
    /// # Errors
    ///
    /// [`StorageError`] if a file exists but cannot be read or parsed. A
    /// corrupt ledger is refused rather than silently treated as empty: an empty
    /// ledger denies everything, which would look like a safe failure but would
    /// quietly discard every decision the user has made.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let layout = StorageLayout::new(root.as_ref());

        let ledger =
            read_json(&layout.root().join(LEDGER_FILE), "permission ledger")?.unwrap_or_default();
        let audit: AuditLog =
            read_json(&layout.root().join(AUDIT_FILE), "audit log")?.unwrap_or_default();

        Ok(Self {
            apps: FileStore::new(layout),
            ledger,
            audit,
        })
    }

    /// Opens the workspace with a redactor already primed with known secrets, so
    /// that nothing sensitive can reach the audit record from the first entry
    /// onwards.
    ///
    /// # Errors
    ///
    /// As [`Workspace::open`].
    pub fn open_with_redactor(
        root: impl AsRef<Path>,
        redactor: Redactor,
    ) -> Result<Self, StorageError> {
        let mut workspace = Self::open(root)?;
        workspace.audit = AuditLog::with_redactor(redactor);
        Ok(workspace)
    }

    /// Where this workspace keeps its files.
    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        self.apps.layout()
    }

    /// The applications.
    #[must_use]
    pub fn apps(&self) -> &FileStore {
        &self.apps
    }

    /// The applications, mutably.
    pub fn apps_mut(&mut self) -> &mut FileStore {
        &mut self.apps
    }

    /// The permission ledger.
    #[must_use]
    pub fn ledger(&self) -> &PermissionLedger {
        &self.ledger
    }

    /// The permission ledger, mutably.
    pub fn ledger_mut(&mut self) -> &mut PermissionLedger {
        &mut self.ledger
    }

    /// The audit record.
    #[must_use]
    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    /// The audit record, mutably.
    pub fn audit_mut(&mut self) -> &mut AuditLog {
        &mut self.audit
    }

    /// Loads every application, skipping any that cannot be read.
    ///
    /// Returns the applications it could load and the ids it could not, so a
    /// caller can show the working ones and still report the broken ones. One
    /// unreadable manifest must not make the whole product refuse to list
    /// anything.
    ///
    /// # Errors
    ///
    /// [`StorageError`] only if the application directory itself cannot be read.
    pub fn load_all(&self) -> Result<LoadedApps, StorageError> {
        let mut result = LoadedApps::default();

        for id in self.apps.list()? {
            match self.apps.load(&id) {
                Ok(manifest) => result.loaded.push(manifest),
                Err(error) => result.broken.push((id, error.to_string())),
            }
        }

        Ok(result)
    }

    /// Persists the ledger and the audit record.
    ///
    /// Applications are saved individually as they change, through
    /// [`Workspace::apps_mut`]; these two are whole-file records and are written
    /// together at the end of an operation.
    ///
    /// # Errors
    ///
    /// [`StorageError`] if either file cannot be written.
    pub fn save(&self) -> Result<(), StorageError> {
        let root = self.layout().root();

        write_atomically(
            &root.join(LEDGER_FILE),
            serialise(&self.ledger, "permission ledger")?.as_bytes(),
        )?;
        write_atomically(
            &root.join(AUDIT_FILE),
            serialise(&self.audit, "audit log")?.as_bytes(),
        )?;

        Ok(())
    }
}

fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    what: &'static str,
) -> Result<Option<T>, StorageError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(StorageError::Io {
                operation: "read",
                path: path.display().to_string(),
                message: error.to_string(),
            });
        }
    };

    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StorageError::Io {
            operation: "parse",
            path: path.display().to_string(),
            message: format!("the {what} is not valid: {error}"),
        })
}

fn serialise<T: serde::Serialize>(value: &T, what: &'static str) -> Result<String, StorageError> {
    serde_json::to_string_pretty(value).map_err(|error| StorageError::Io {
        operation: "serialise",
        path: what.to_owned(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        actor::Actor,
        identity::Principal,
        manifest::RuntimeSpec,
        permission::{AppPermission, Decision, MetaPermission, PathScope, Permission},
    };
    use tempfile::TempDir;

    fn id(value: &str) -> AppId {
        AppId::parse(value).unwrap()
    }

    fn manifest(value: &str) -> AppManifest {
        AppManifest::new(
            id(value),
            "Test App",
            RuntimeSpec::docker_job("alpine", vec!["true".to_owned()]),
        )
    }

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    #[test]
    fn a_fresh_device_opens_empty_rather_than_failing() {
        let directory = TempDir::new().unwrap();
        let workspace = Workspace::open(directory.path().join("never-used")).unwrap();

        assert!(workspace.apps().list().unwrap().is_empty());
        assert!(workspace.ledger().grants().is_empty());
        assert!(workspace.audit().is_empty());
    }

    #[test]
    fn everything_survives_a_round_trip() {
        let directory = TempDir::new().unwrap();

        {
            let mut workspace = Workspace::open(directory.path()).unwrap();
            workspace
                .apps_mut()
                .save(&manifest("csv-comparator"))
                .unwrap();
            workspace
                .ledger_mut()
                .allow(
                    Principal::app(id("csv-comparator")),
                    AppPermission::read(scope("~/Downloads/**")),
                    Actor::User,
                    "to compare your files",
                )
                .unwrap();
            workspace.audit_mut().append(
                Actor::User,
                crate::audit::AuditEvent::AppCreated {
                    app: id("csv-comparator"),
                    purpose: "compare two exports".to_owned(),
                },
            );
            workspace.save().unwrap();
        }

        let workspace = Workspace::open(directory.path()).unwrap();

        assert_eq!(workspace.apps().list().unwrap(), vec![id("csv-comparator")]);
        assert_eq!(
            workspace.ledger().check(
                &Principal::app(id("csv-comparator")),
                &Permission::App(AppPermission::read(scope("~/Downloads/a.csv")))
            ),
            Decision::Allow
        );
        assert_eq!(workspace.audit().len(), 1);
        workspace.audit().verify().unwrap();
    }

    /// A revocation that did not survive a restart would silently restore an
    /// access the user had taken away.
    #[test]
    fn a_revocation_survives_a_restart() {
        let directory = TempDir::new().unwrap();

        {
            let mut workspace = Workspace::open(directory.path()).unwrap();
            workspace
                .ledger_mut()
                .allow(
                    Principal::app(id("csv-comparator")),
                    AppPermission::Camera,
                    Actor::User,
                    "it asked",
                )
                .unwrap();
            workspace
                .ledger_mut()
                .revoke_all(&Principal::app(id("csv-comparator")));
            workspace.save().unwrap();
        }

        let workspace = Workspace::open(directory.path()).unwrap();
        assert_eq!(
            workspace.ledger().check(
                &Principal::app(id("csv-comparator")),
                &Permission::App(AppPermission::Camera)
            ),
            Decision::Deny
        );
    }

    /// A corrupt ledger must not be read as an empty one. An empty ledger denies
    /// everything, which looks like a safe failure — but it would quietly throw
    /// away every decision the user had made, and they would be asked to make
    /// them all again with no indication anything was lost.
    #[test]
    fn a_corrupt_ledger_is_refused_rather_than_treated_as_empty() {
        let directory = TempDir::new().unwrap();
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(directory.path().join(LEDGER_FILE), "{ not json").unwrap();

        let error = Workspace::open(directory.path()).unwrap_err();
        assert!(matches!(error, StorageError::Io { .. }));
        assert!(error.to_string().contains("permission ledger"));
    }

    #[test]
    fn a_corrupt_audit_log_is_refused_too() {
        let directory = TempDir::new().unwrap();
        std::fs::create_dir_all(directory.path()).unwrap();
        std::fs::write(directory.path().join(AUDIT_FILE), "[]").unwrap();

        assert!(Workspace::open(directory.path()).is_err());
    }

    /// One unreadable manifest must not stop the product listing everything
    /// else — but it must still be reported, not hidden.
    #[test]
    fn loading_reports_broken_applications_without_hiding_the_working_ones() {
        let directory = TempDir::new().unwrap();
        let mut workspace = Workspace::open(directory.path()).unwrap();
        workspace.apps_mut().save(&manifest("good-app")).unwrap();
        workspace.apps_mut().save(&manifest("broken-app")).unwrap();

        std::fs::write(
            workspace.layout().app(&id("broken-app")).manifest(),
            "{ not json",
        )
        .unwrap();

        let result = workspace.load_all().unwrap();

        assert_eq!(result.loaded.len(), 1);
        assert_eq!(result.loaded[0].id, id("good-app"));
        assert_eq!(result.broken.len(), 1);
        assert_eq!(result.broken[0].0, id("broken-app"));
    }

    #[test]
    fn secrets_registered_at_open_never_reach_the_record() {
        let directory = TempDir::new().unwrap();
        let mut redactor = Redactor::new();
        redactor.register_secret("a-registered-api-key");

        let mut workspace = Workspace::open_with_redactor(directory.path(), redactor).unwrap();
        workspace.audit_mut().append(
            Actor::Ephemeral,
            crate::audit::AuditEvent::AppCreated {
                app: id("leaky"),
                purpose: "use a-registered-api-key".to_owned(),
            },
        );
        workspace.save().unwrap();

        let written = std::fs::read_to_string(directory.path().join(AUDIT_FILE)).unwrap();
        assert!(!written.contains("a-registered-api-key"), "{written}");
    }

    /// Ephemeral's own grants and an application's live in the same ledger file
    /// but must stay separate principals across a restart.
    #[test]
    fn the_two_permission_systems_stay_separate_across_a_restart() {
        let directory = TempDir::new().unwrap();

        {
            let mut workspace = Workspace::open(directory.path()).unwrap();
            workspace
                .ledger_mut()
                .allow(
                    Principal::Ephemeral,
                    MetaPermission::read(scope("~/**")),
                    Actor::User,
                    "setup",
                )
                .unwrap();
            workspace.save().unwrap();
        }

        let workspace = Workspace::open(directory.path()).unwrap();
        assert_eq!(
            workspace.ledger().check(
                &Principal::app(id("any-app")),
                &Permission::App(AppPermission::read(scope("~/anything")))
            ),
            Decision::Deny,
            "an application must inherit nothing, restart or not"
        );
    }
}
