//! A directory an application may look at and may not change.
//!
//! ## Why this exists
//!
//! Every mount used to be handed to WASI as a `cap_std` directory opened with
//! ambient authority — full rights, whatever the specification said about the
//! mount. The `writable` flag on a [`crate::spec::Mount`] was computed into a
//! single summary, shown to a person, and never consulted when the directory
//! was opened. So under this runtime **every mount was read-write**.
//!
//! That is not a theoretical gap. It was found by revoking an application's
//! write permission and watching it write anyway, while the run's own banner
//! correctly said *"Can read /…"* and the sentence shown when granting read
//! says, in these words: *"It cannot change those files."*
//!
//! ## What it refuses
//!
//! Everything that changes anything: opening a file for writing, creating,
//! truncating, deleting, renaming, linking, symlinking, making a directory, and
//! setting times. Reading, listing, `stat` and following a symlink are passed
//! through to the real directory unchanged.
//!
//! The refusal is WASI's own `EPERM`, which is what a program's own error
//! handling already knows how to read — the reference application turns it into
//! *"this application may not have been allowed to write to it"* without
//! knowing anything about Ephemeral.

use std::any::Any;
use std::path::PathBuf;

use wasmi_wasi::wasi_common::dir::{OpenResult, ReaddirCursor, ReaddirEntity, WasiDir};
use wasmi_wasi::wasi_common::file::{FdFlags, Filestat, OFlags};
use wasmi_wasi::wasi_common::{Error, ErrorExt as _, SystemTimeSpec};

/// Wraps a directory so that nothing inside it can be changed.
pub(crate) struct ReadOnly(pub(crate) Box<dyn WasiDir>);

impl ReadOnly {
    /// The refusal every mutating operation returns.
    ///
    /// `EPERM` rather than `ENOTSUP`: the operation is one this filesystem
    /// supports perfectly well and this application is not permitted to do,
    /// which is exactly what "not permitted" means.
    fn refused() -> Error {
        Error::perm()
    }
}

#[async_trait::async_trait]
impl WasiDir for ReadOnly {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Opens a file, so long as opening it changes nothing.
    ///
    /// Both halves matter. `write` covers the obvious case; the `oflags` cover
    /// the ones that are easy to miss — `CREATE` makes a file that was not
    /// there, and `TRUNCATE` empties one that was, and a caller can ask for
    /// either while claiming to only want to read.
    async fn open_file(
        &self,
        symlink_follow: bool,
        path: &str,
        oflags: OFlags,
        read: bool,
        write: bool,
        fdflags: FdFlags,
    ) -> Result<OpenResult, Error> {
        let changes = write
            || oflags.contains(OFlags::CREATE)
            || oflags.contains(OFlags::TRUNCATE)
            || oflags.contains(OFlags::EXCLUSIVE)
            || fdflags.contains(FdFlags::APPEND);

        if changes {
            return Err(Self::refused());
        }

        // A directory opened through a read-only one is read-only too.
        // Otherwise the whole guarantee lasts exactly one level deep.
        match self
            .0
            .open_file(symlink_follow, path, oflags, read, write, fdflags)
            .await?
        {
            OpenResult::Dir(dir) => Ok(OpenResult::Dir(Box::new(Self(dir)))),
            file @ OpenResult::File(_) => Ok(file),
        }
    }

    async fn readdir(
        &self,
        cursor: ReaddirCursor,
    ) -> Result<Box<dyn Iterator<Item = Result<ReaddirEntity, Error>> + Send>, Error> {
        self.0.readdir(cursor).await
    }

    async fn read_link(&self, path: &str) -> Result<PathBuf, Error> {
        self.0.read_link(path).await
    }

    async fn get_filestat(&self) -> Result<Filestat, Error> {
        self.0.get_filestat().await
    }

    async fn get_path_filestat(
        &self,
        path: &str,
        follow_symlinks: bool,
    ) -> Result<Filestat, Error> {
        self.0.get_path_filestat(path, follow_symlinks).await
    }

    // Everything below changes something, and is refused. Written out one by
    // one rather than left to the trait's defaults: a default that returned
    // "not supported" would be indistinguishable from a filesystem that cannot
    // do it, and a method added to `WasiDir` later would silently inherit
    // whatever the default happened to be.

    async fn create_dir(&self, _path: &str) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn symlink(&self, _old_path: &str, _new_path: &str) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn remove_dir(&self, _path: &str) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn unlink_file(&self, _path: &str) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn rename(
        &self,
        _path: &str,
        _dest_dir: &dyn WasiDir,
        _dest_path: &str,
    ) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn hard_link(
        &self,
        _path: &str,
        _target_dir: &dyn WasiDir,
        _target_path: &str,
    ) -> Result<(), Error> {
        Err(Self::refused())
    }

    async fn set_times(
        &self,
        _path: &str,
        _atime: Option<SystemTimeSpec>,
        _mtime: Option<SystemTimeSpec>,
        _follow_symlinks: bool,
    ) -> Result<(), Error> {
        Err(Self::refused())
    }
}
