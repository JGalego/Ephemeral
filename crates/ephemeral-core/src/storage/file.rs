//! The filesystem-backed application store.
//!
//! Available with the `fs` feature, which is on by default. This is the only
//! module in the crate that touches the host, which is what makes
//! `--no-default-features` a meaningful check rather than a slogan.
//!
//! ## Why writes are atomic
//!
//! A manifest is the record of what an application is *allowed to do*. A
//! half-written one — from a crash, a full disk, or a machine losing power — is
//! not merely inconvenient: on the next read it is either a parse failure or, in
//! the worst case, a document that parses into something nobody approved.
//!
//! So every write goes to a temporary file in the same directory and is renamed
//! into place. A reader sees the old manifest or the new one, never a mixture.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use super::{AppStore, StorageError, StorageLayout};
use crate::{identity::AppId, manifest::AppManifest};

/// An application store backed by a directory on this device.
///
/// Local-first by construction: no server, no daemon, and a layout a person can
/// open in a file browser and understand.
#[derive(Debug, Clone)]
pub struct FileStore {
    layout: StorageLayout,
}

impl FileStore {
    /// A store rooted at a directory.
    ///
    /// Creating the store does not create the directory; the first write does.
    #[must_use]
    pub fn new(layout: StorageLayout) -> Self {
        Self { layout }
    }

    /// A store rooted at a path.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self::new(StorageLayout::new(root))
    }

    /// The layout this store uses.
    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    /// Creates the directory structure for an application.
    ///
    /// Called before generation begins, so that every stage writes into a place
    /// that already exists and is known to be inside the application's own tree.
    ///
    /// # Errors
    ///
    /// [`StorageError::Io`] if a directory cannot be created.
    pub fn prepare(&self, id: &AppId) -> Result<(), StorageError> {
        for directory in self.layout.app(id).directories() {
            create_dir_all(&directory)?;
        }
        Ok(())
    }

    /// Removes everything belonging to an application, irreversibly.
    ///
    /// This is *purge*, not delete. Deleting an application withdraws its
    /// capability and moves it to the trash; purging is what a user asks for
    /// when they mean it, and there is no way back afterwards.
    ///
    /// # Errors
    ///
    /// [`StorageError::Io`] if the tree cannot be removed. Removing something
    /// that is already gone succeeds.
    pub fn purge(&self, id: &AppId) -> Result<(), StorageError> {
        for root in [
            self.layout.app(id).root().to_path_buf(),
            self.layout.trashed_app(id).root().to_path_buf(),
        ] {
            if root.exists() {
                fs::remove_dir_all(&root).map_err(|error| io_error("remove", &root, &error))?;
            }
        }
        Ok(())
    }

    /// Moves an application's files into the trash, where they wait to be
    /// purged.
    ///
    /// # Errors
    ///
    /// [`StorageError::NotFound`] if the application has no files, or
    /// [`StorageError::Io`] if they cannot be moved.
    pub fn move_to_trash(&self, id: &AppId) -> Result<(), StorageError> {
        let from = self.layout.app(id).root().to_path_buf();
        if !from.exists() {
            return Err(StorageError::NotFound { id: id.clone() });
        }

        let to = self.layout.trashed_app(id).root().to_path_buf();
        if to.exists() {
            fs::remove_dir_all(&to).map_err(|error| io_error("remove", &to, &error))?;
        }
        if let Some(parent) = to.parent() {
            create_dir_all(parent)?;
        }

        fs::rename(&from, &to).map_err(|error| io_error("move", &from, &error))
    }

    fn manifest_path(&self, id: &AppId) -> PathBuf {
        self.layout.app(id).manifest()
    }
}

impl AppStore for FileStore {
    fn save(&mut self, manifest: &AppManifest) -> Result<(), StorageError> {
        manifest
            .validate()
            .map_err(|error| StorageError::InvalidManifest {
                id: manifest.id.clone(),
                problem: error.to_string(),
            })?;

        let path = self.manifest_path(&manifest.id);
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        create_dir_all(directory)?;

        let json = manifest
            .to_json()
            .map_err(|error| StorageError::InvalidManifest {
                id: manifest.id.clone(),
                problem: error.to_string(),
            })?;

        write_atomically(&path, json.as_bytes())
    }

    fn load(&self, id: &AppId) -> Result<AppManifest, StorageError> {
        let path = self.manifest_path(id);

        let json = match fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound { id: id.clone() });
            }
            Err(error) => return Err(io_error("read", &path, &error)),
        };

        let manifest =
            AppManifest::from_json(&json).map_err(|error| StorageError::InvalidManifest {
                id: id.clone(),
                problem: error.to_string(),
            })?;

        // The directory name and the manifest's own id must agree. A mismatch
        // means the file was moved or edited, and acting on it would apply one
        // application's permissions under another's identity.
        if &manifest.id != id {
            return Err(StorageError::IdentityMismatch {
                expected: id.clone(),
                found: manifest.id,
            });
        }

        Ok(manifest)
    }

    fn list(&self) -> Result<Vec<AppId>, StorageError> {
        let apps_dir = self.layout.apps_dir();

        let entries = match fs::read_dir(&apps_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("read", &apps_dir, &error)),
        };

        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error("read", &apps_dir, &error))?;
            if !entry.path().join(super::MANIFEST_FILE).is_file() {
                continue;
            }
            // A directory whose name is not a valid id was not written by us.
            // Skipping it is right: refusing to list anything because of one
            // stray directory would make the product unusable over a typo.
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| AppId::parse(name).ok())
            {
                ids.push(id);
            }
        }

        ids.sort();
        Ok(ids)
    }

    fn remove(&mut self, id: &AppId) -> Result<(), StorageError> {
        let path = self.manifest_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(StorageError::NotFound { id: id.clone() })
            }
            Err(error) => Err(io_error("remove", &path, &error)),
        }
    }
}

/// Writes a file so that a reader sees either the old contents or the new ones,
/// never a mixture.
///
/// Used for every record Ephemeral persists — manifests, the permission ledger,
/// the audit log. All three answer "what is this application allowed to do", and
/// a half-written answer to that question is worse than no answer: on the next
/// read it is either a parse failure or, at worst, a document nobody approved.
///
/// The temporary file is created in the destination directory so the rename
/// stays within one filesystem, which is what makes it atomic.
pub(crate) fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), StorageError> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    create_dir_all(directory)?;

    let mut temporary =
        NamedTempFile::new_in(directory).map_err(|e| io_error("write into", directory, &e))?;
    temporary
        .write_all(contents)
        .map_err(|e| io_error("write", path, &e))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|e| io_error("flush", path, &e))?;
    temporary
        .persist(path)
        .map_err(|e| io_error("save", path, &e.error))?;

    Ok(())
}

fn create_dir_all(path: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(path).map_err(|error| io_error("create", path, &error))
}

fn io_error(operation: &'static str, path: &Path, error: &std::io::Error) -> StorageError {
    StorageError::Io {
        operation,
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RuntimeSpec;
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

    fn store() -> (TempDir, FileStore) {
        let directory = TempDir::new().unwrap();
        let store = FileStore::at(directory.path());
        (directory, store)
    }

    #[test]
    fn applications_round_trip_through_the_filesystem() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");

        store.save(&app).unwrap();

        assert_eq!(store.load(&app.id).unwrap(), app);
        assert_eq!(store.list().unwrap(), vec![app.id.clone()]);
        assert!(store.layout().app(&app.id).manifest().is_file());
    }

    #[test]
    fn listing_an_uninitialised_store_is_empty_rather_than_an_error() {
        let (_directory, store) = store();
        assert_eq!(store.list().unwrap(), Vec::new());
        assert!(matches!(
            store.load(&id("missing")),
            Err(StorageError::NotFound { .. })
        ));
    }

    #[test]
    fn preparing_an_application_creates_its_whole_layout() {
        let (_directory, store) = store();
        let app = id("csv-comparator");

        store.prepare(&app).unwrap();

        let paths = store.layout().app(&app);
        for directory in paths.directories() {
            assert!(
                directory.is_dir(),
                "{} was not created",
                directory.display()
            );
        }
    }

    /// The manifest written to disk must be the readable, portable document the
    /// user is promised, not an internal encoding.
    #[test]
    fn the_stored_manifest_is_readable_json() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");
        store.save(&app).unwrap();

        let raw = fs::read_to_string(store.layout().app(&app.id).manifest()).unwrap();
        assert!(raw.contains("\"schema_version\": 1"), "{raw}");
        assert!(raw.contains("\"id\": \"csv-comparator\""), "{raw}");
    }

    /// A manifest that no longer validates is refused rather than partly
    /// applied — it is the document that says what an application may do.
    #[test]
    fn a_corrupted_manifest_is_refused() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");
        store.save(&app).unwrap();

        fs::write(store.layout().app(&app.id).manifest(), "{ not json").unwrap();
        assert!(matches!(
            store.load(&app.id),
            Err(StorageError::InvalidManifest { .. })
        ));
    }

    /// Moving one application's manifest into another's directory must not let
    /// it act under that identity.
    #[test]
    fn a_manifest_in_the_wrong_directory_is_refused() {
        let (_directory, mut store) = store();
        let real = manifest("app-a");
        store.save(&real).unwrap();
        store.prepare(&id("app-b")).unwrap();

        let stolen = fs::read_to_string(store.layout().app(&real.id).manifest()).unwrap();
        fs::write(store.layout().app(&id("app-b")).manifest(), stolen).unwrap();

        let error = store.load(&id("app-b")).unwrap_err();
        assert_eq!(
            error,
            StorageError::IdentityMismatch {
                expected: id("app-b"),
                found: id("app-a"),
            }
        );
    }

    /// A stray directory should not stop the product listing everything else.
    #[test]
    fn unrecognised_directories_are_skipped_rather_than_fatal() {
        let (_directory, mut store) = store();
        store.save(&manifest("csv-comparator")).unwrap();

        fs::create_dir_all(store.layout().apps_dir().join("Not A Valid Id")).unwrap();
        fs::create_dir_all(store.layout().apps_dir().join("no-manifest-here")).unwrap();

        assert_eq!(store.list().unwrap(), vec![id("csv-comparator")]);
    }

    #[test]
    fn saving_twice_replaces_the_stored_manifest() {
        let (_directory, mut store) = store();
        let mut app = manifest("csv-comparator");
        store.save(&app).unwrap();

        app.description = "changed".to_owned();
        app.version = 2;
        store.save(&app).unwrap();

        let loaded = store.load(&app.id).unwrap();
        assert_eq!(loaded.description, "changed");
        assert_eq!(loaded.version, 2);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    /// Deleting an application moves its files aside so the user can change
    /// their mind; purging is what actually destroys them ([ADR-0009]).
    ///
    /// [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md
    #[test]
    fn deletion_moves_files_to_the_trash_and_purging_destroys_them() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");
        store.prepare(&app.id).unwrap();
        store.save(&app).unwrap();
        fs::write(
            store.layout().app(&app.id).data().join("result.csv"),
            "data",
        )
        .unwrap();

        store.move_to_trash(&app.id).unwrap();

        assert!(!store.layout().app(&app.id).root().exists());
        assert!(
            store
                .layout()
                .trashed_app(&app.id)
                .data()
                .join("result.csv")
                .is_file(),
            "the user's data must survive deletion until they purge"
        );

        store.purge(&app.id).unwrap();

        assert!(!store.layout().trashed_app(&app.id).root().exists());
        assert!(!store.layout().app(&app.id).root().exists());
    }

    /// Purge must be complete: nothing belonging to the application survives it,
    /// in either location.
    #[test]
    fn purging_removes_everything_including_what_was_never_trashed() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");
        store.prepare(&app.id).unwrap();
        store.save(&app).unwrap();
        fs::write(store.layout().app(&app.id).logs().join("build.log"), "log").unwrap();

        store.purge(&app.id).unwrap();

        assert!(!store.layout().app(&app.id).root().exists());
        assert_eq!(store.list().unwrap(), Vec::new());
        assert!(matches!(
            store.load(&app.id),
            Err(StorageError::NotFound { .. })
        ));
    }

    #[test]
    fn purging_something_that_is_already_gone_succeeds() {
        let (_directory, store) = store();
        store.purge(&id("never-existed")).unwrap();
    }

    /// One application's files are untouched by another's deletion.
    #[test]
    fn purging_one_application_leaves_the_others_alone() {
        let (_directory, mut store) = store();
        for value in ["app-a", "app-b"] {
            store.prepare(&id(value)).unwrap();
            store.save(&manifest(value)).unwrap();
        }

        store.purge(&id("app-a")).unwrap();

        assert_eq!(store.list().unwrap(), vec![id("app-b")]);
        assert!(store.layout().app(&id("app-b")).root().exists());
    }

    /// A crash during a write must leave the previous manifest readable, not a
    /// half-written document describing permissions nobody approved.
    #[test]
    fn a_failed_write_leaves_no_partial_manifest() {
        let (_directory, mut store) = store();
        let app = manifest("csv-comparator");
        store.save(&app).unwrap();

        let broken = AppManifest {
            name: String::new(),
            ..app.clone()
        };
        assert!(store.save(&broken).is_err());

        assert_eq!(
            store.load(&app.id).unwrap(),
            app,
            "the previously stored manifest must be intact"
        );
    }
}
