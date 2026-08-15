//! Where applications are kept, and how they are read back.
//!
//! Two things live here: the [`StorageLayout`] — which is pure path arithmetic
//! and available everywhere — and the [`AppStore`] trait with its
//! implementations.
//!
//! [`MemoryStore`] is always available and is what tests use. [`FileStore`]
//! needs the `fs` feature, which is on by default; turning it off leaves this
//! crate performing no host I/O at all, and CI builds it that way to keep the
//! boundary honest ([ADR-0002]).
//!
//! [ADR-0002]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0002-rust-core-with-platform-shells.md

mod layout;

#[cfg(feature = "fs")]
mod file;

pub use layout::{APPS_DIR, AUDIT_FILE, AppPaths, MANIFEST_FILE, StorageLayout, TRASH_DIR};

#[cfg(feature = "fs")]
pub use file::FileStore;

use std::collections::BTreeMap;

use crate::{identity::AppId, manifest::AppManifest};

/// Why a storage operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// No application with that id is stored.
    #[error("no application with id {id} is stored")]
    NotFound {
        /// The id that was looked up.
        id: AppId,
    },

    /// An application with that id is already stored.
    #[error("an application with id {id} is already stored")]
    AlreadyExists {
        /// The id that collided.
        id: AppId,
    },

    /// The stored manifest could not be read or is not valid.
    ///
    /// A manifest on disk is a document a user can edit, so a stored application
    /// that no longer validates is refused rather than partly applied.
    #[error("the stored manifest for {id} is not valid: {problem}")]
    InvalidManifest {
        /// Which application.
        id: AppId,
        /// What is wrong with it.
        problem: String,
    },

    /// The stored id does not match the manifest inside it.
    ///
    /// A mismatch means the manifest was moved or edited, and acting on it would
    /// apply one application's permissions under another's identity.
    #[error("the manifest stored as {expected} claims to be {found}")]
    IdentityMismatch {
        /// The id it was stored under.
        expected: AppId,
        /// The id the manifest claims.
        found: AppId,
    },

    /// The filesystem refused an operation.
    #[error("could not {operation} {path}: {message}")]
    Io {
        /// What was being attempted.
        operation: &'static str,
        /// Which path.
        path: String,
        /// What the operating system said.
        message: String,
    },
}

/// Somewhere applications can be saved and read back.
///
/// The interface the rest of the system uses, so that the desktop application,
/// the CLI and the tests all speak to storage the same way and none of them
/// touches a path directly.
pub trait AppStore {
    /// Saves an application, creating it or replacing what is there.
    ///
    /// # Errors
    ///
    /// [`StorageError`] if the manifest is not valid or cannot be written.
    fn save(&mut self, manifest: &AppManifest) -> Result<(), StorageError>;

    /// Reads an application back.
    ///
    /// # Errors
    ///
    /// [`StorageError::NotFound`] if there is no such application, or
    /// [`StorageError::InvalidManifest`] / [`StorageError::IdentityMismatch`] if
    /// what is stored cannot be trusted.
    fn load(&self, id: &AppId) -> Result<AppManifest, StorageError>;

    /// Every stored application, in a stable order.
    ///
    /// # Errors
    ///
    /// [`StorageError`] if storage cannot be read.
    fn list(&self) -> Result<Vec<AppId>, StorageError>;

    /// Removes an application's record.
    ///
    /// This is the record only. Destroying an application's *files* is the
    /// purge operation, and destroying its runtime resources is the runtime's
    /// job — both happen before this.
    ///
    /// # Errors
    ///
    /// [`StorageError::NotFound`] if there is no such application.
    fn remove(&mut self, id: &AppId) -> Result<(), StorageError>;

    /// Whether an application is stored.
    ///
    /// # Errors
    ///
    /// [`StorageError`] if storage cannot be read.
    fn contains(&self, id: &AppId) -> Result<bool, StorageError> {
        match self.load(id) {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Saves an application that must not already exist.
    ///
    /// # Errors
    ///
    /// [`StorageError::AlreadyExists`] if it does, or anything
    /// [`AppStore::save`] returns.
    fn create(&mut self, manifest: &AppManifest) -> Result<(), StorageError> {
        if self.contains(&manifest.id)? {
            return Err(StorageError::AlreadyExists {
                id: manifest.id.clone(),
            });
        }
        self.save(manifest)
    }
}

/// An application store held in memory.
///
/// What tests use, and what a future in-memory preview mode would use. Applies
/// exactly the same validation as [`FileStore`], so a test that passes here is
/// testing the same rules that hold on disk.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    apps: BTreeMap<AppId, AppManifest>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many applications are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }
}

impl AppStore for MemoryStore {
    fn save(&mut self, manifest: &AppManifest) -> Result<(), StorageError> {
        manifest
            .validate()
            .map_err(|error| StorageError::InvalidManifest {
                id: manifest.id.clone(),
                problem: error.to_string(),
            })?;

        self.apps.insert(manifest.id.clone(), manifest.clone());
        Ok(())
    }

    fn load(&self, id: &AppId) -> Result<AppManifest, StorageError> {
        self.apps
            .get(id)
            .cloned()
            .ok_or_else(|| StorageError::NotFound { id: id.clone() })
    }

    fn list(&self) -> Result<Vec<AppId>, StorageError> {
        Ok(self.apps.keys().cloned().collect())
    }

    fn remove(&mut self, id: &AppId) -> Result<(), StorageError> {
        self.apps
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| StorageError::NotFound { id: id.clone() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RuntimeSpec;

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

    #[test]
    fn an_empty_store_holds_nothing() {
        let store = MemoryStore::new();
        assert!(store.is_empty());
        assert_eq!(store.list().unwrap(), Vec::new());
        assert!(!store.contains(&id("missing")).unwrap());
        assert!(matches!(
            store.load(&id("missing")),
            Err(StorageError::NotFound { .. })
        ));
    }

    #[test]
    fn applications_round_trip_through_the_store() {
        let mut store = MemoryStore::new();
        let app = manifest("csv-comparator");

        store.save(&app).unwrap();

        assert_eq!(store.load(&app.id).unwrap(), app);
        assert!(store.contains(&app.id).unwrap());
        assert_eq!(store.list().unwrap(), vec![app.id.clone()]);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn saving_twice_replaces_and_creating_twice_refuses() {
        let mut store = MemoryStore::new();
        let mut app = manifest("csv-comparator");

        store.create(&app).unwrap();

        app.description = "changed".to_owned();
        store.save(&app).unwrap();
        assert_eq!(store.load(&app.id).unwrap().description, "changed");

        assert!(matches!(
            store.create(&app),
            Err(StorageError::AlreadyExists { .. })
        ));
    }

    /// An invalid manifest must not reach storage, or the next process to read
    /// it inherits the problem.
    #[test]
    fn an_invalid_manifest_is_refused() {
        let mut store = MemoryStore::new();
        let broken = AppManifest {
            name: String::new(),
            ..manifest("csv-comparator")
        };

        assert!(matches!(
            store.save(&broken),
            Err(StorageError::InvalidManifest { .. })
        ));
        assert!(store.is_empty());
    }

    #[test]
    fn removing_takes_the_record_away() {
        let mut store = MemoryStore::new();
        let app = manifest("csv-comparator");
        store.save(&app).unwrap();

        store.remove(&app.id).unwrap();

        assert!(store.is_empty());
        assert!(matches!(
            store.remove(&app.id),
            Err(StorageError::NotFound { .. })
        ));
    }

    /// Storing one application must not affect another. The store is the record
    /// half of cross-application isolation.
    #[test]
    fn applications_do_not_interfere_with_each_other() {
        let mut store = MemoryStore::new();
        let a = manifest("app-a");
        let b = manifest("app-b");

        store.save(&a).unwrap();
        store.save(&b).unwrap();
        store.remove(&a.id).unwrap();

        assert!(!store.contains(&a.id).unwrap());
        assert_eq!(store.load(&b.id).unwrap(), b);
    }

    #[test]
    fn listing_is_in_a_stable_order() {
        let mut store = MemoryStore::new();
        for value in ["zebra", "alpha", "middle"] {
            store.save(&manifest(value)).unwrap();
        }

        assert_eq!(
            store.list().unwrap(),
            vec![id("alpha"), id("middle"), id("zebra")]
        );
    }
}
