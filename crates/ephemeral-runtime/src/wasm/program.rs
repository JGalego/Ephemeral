//! What there is to run, and how a phone with no toolchain gets one.
//!
//! [`super::run`] takes bytes. This module is the part that decides *which*
//! bytes, and it exists because "the application has to be WebAssembly" is a
//! sentence with two honest readings.
//!
//! An application can **be** a module — compiled somewhere with a toolchain,
//! published, and run anywhere. That is the fast path and the one a desktop
//! can produce.
//!
//! An application can also be a **script**, run by an interpreter that is
//! itself a module. Nothing on a handset can compile anything, so this is the
//! only reading under which a phone can run something a model wrote ten seconds
//! ago. The interpreter is ordinary confined code here: it gets the same
//! preopens, the same fuel and the same absence of sockets as anything else,
//! and being an interpreter buys it nothing.

use std::path::{Path, PathBuf};

/// Where a script is mounted for its interpreter, as the module sees it.
///
/// A fixed name rather than the host path: the module is told where its own
/// program is, and the host's directory layout is not something a generated
/// application has any business learning.
pub const PROGRAM_DIRECTORY: &str = "/program";

/// What to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// The application is itself a WebAssembly module.
    Module {
        /// The `.wasm` file.
        module: PathBuf,
    },

    /// The application is a script, and something else runs it.
    Interpreted {
        /// The interpreter, which is a WebAssembly module.
        interpreter: PathBuf,

        /// The script's directory on the host, mounted read-only.
        ///
        /// The directory rather than the file, so that a program written as
        /// several files can still reach its own siblings. It is the
        /// application's own source directory, so what that exposes is the
        /// application to itself.
        source: PathBuf,

        /// The file inside it to run, by name.
        entry: String,
    },
}

impl Program {
    /// An application that is a module.
    #[must_use]
    pub fn module(path: impl Into<PathBuf>) -> Self {
        Self::Module {
            module: path.into(),
        }
    }

    /// An application that is a script, and the interpreter that runs it.
    ///
    /// Returns `None` if `script` has no file name or no parent directory,
    /// which is not a case to guess at: a script that cannot be named is one
    /// whose interpreter would be handed something arbitrary.
    #[must_use]
    pub fn interpreted(interpreter: impl Into<PathBuf>, script: &Path) -> Option<Self> {
        Some(Self::Interpreted {
            interpreter: interpreter.into(),
            source: script.parent()?.to_path_buf(),
            entry: script.file_name()?.to_string_lossy().into_owned(),
        })
    }

    /// The module to load and run.
    #[must_use]
    pub fn wasm(&self) -> &Path {
        match self {
            Self::Module { module } => module,
            Self::Interpreted { interpreter, .. } => interpreter,
        }
    }

    /// What has to be visible for the program to find itself, if anything.
    ///
    /// Read-only, always. An interpreter that could rewrite the script it is
    /// running is an application that can edit itself between runs, and an
    /// application's source is the one thing a version digest promises has not
    /// changed ([ADR-0011]).
    ///
    /// [ADR-0011]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0011-immutable-content-addressed-versions.md
    #[must_use]
    pub fn mounted(&self) -> Option<(PathBuf, String)> {
        match self {
            Self::Module { .. } => None,
            Self::Interpreted { source, .. } => {
                Some((source.clone(), PROGRAM_DIRECTORY.to_owned()))
            }
        }
    }

    /// The arguments that come before the application's own.
    ///
    /// Empty for a module, because a module *is* the program. For a script it
    /// is the one path the interpreter needs, phrased in the module's own view
    /// of the world.
    #[must_use]
    pub fn leading_arguments(&self) -> Vec<String> {
        match self {
            Self::Module { .. } => Vec::new(),
            Self::Interpreted { entry, .. } => vec![format!("{PROGRAM_DIRECTORY}/{entry}")],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_module_is_its_own_program_and_needs_nothing_mounted() {
        let program = Program::module("/apps/tally/program.wasm");

        assert_eq!(program.wasm(), Path::new("/apps/tally/program.wasm"));
        assert_eq!(program.mounted(), None);
        assert!(program.leading_arguments().is_empty());
    }

    /// The interpreter is what runs, and the script is what it is pointed at.
    /// Getting this the wrong way round would load a text file as a module.
    #[test]
    fn an_interpreted_program_runs_the_interpreter_and_is_shown_the_script() {
        let program = Program::interpreted(
            "/interpreters/js.wasm",
            Path::new("/apps/tally/source/main.js"),
        )
        .expect("a named script in a directory");

        assert_eq!(program.wasm(), Path::new("/interpreters/js.wasm"));
        assert_eq!(
            program.mounted(),
            Some((PathBuf::from("/apps/tally/source"), "/program".to_owned()))
        );
        assert_eq!(program.leading_arguments(), vec!["/program/main.js"]);
    }

    /// The host's directory layout is not something a generated application
    /// learns. It is told where its own program is, in its own terms.
    #[test]
    fn the_script_is_named_in_the_modules_own_view_not_the_hosts() {
        let program = Program::interpreted(
            "/interpreters/js.wasm",
            Path::new("/home/somebody/.ephemeral/apps/tally/source/main.js"),
        )
        .expect("a named script in a directory");

        let argument = program.leading_arguments().remove(0);

        assert!(
            !argument.contains("somebody"),
            "the host's paths must not reach the module: {argument}"
        );
    }

    #[test]
    fn a_script_that_cannot_be_named_is_refused_rather_than_guessed_at() {
        assert!(Program::interpreted("/interpreters/js.wasm", Path::new("/")).is_none());
    }
}
