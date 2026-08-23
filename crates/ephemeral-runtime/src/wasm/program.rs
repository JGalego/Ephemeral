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

/// The interpreters this build knows how to ask for.
///
/// Extension, the language's name for a person, and the file to look for. A
/// table rather than a match so that "which languages can this device run" is
/// a question with one answer in one place — the interface asks it, the error
/// messages quote it, and a model writing an application is told from it.
const INTERPRETERS: &[(&str, &str, &str)] = &[
    ("js", "JavaScript", "javascript.wasm"),
    ("mjs", "JavaScript", "javascript.wasm"),
    ("py", "Python", "python.wasm"),
];

/// Why an application has nothing this runtime can run.
///
/// Each of these is a thing somebody can fix, so each says what and where.
/// "The application could not be started" is the message this type exists to
/// never produce.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NoProgram {
    /// The manifest does not say what to run.
    #[error(
        "this application does not say which file to run, so there is nothing to start. \
         It was probably generated for a container rather than for this device."
    )]
    NotDeclared,

    /// The declared name points outside the application.
    #[error(
        "this application says to run `{named}`, which is not inside it. \
         Nothing will be started."
    )]
    Escapes {
        /// What the manifest said.
        named: String,
    },

    /// The declared file is not there.
    #[error("`{named}` is missing from this application, so there is nothing to run.")]
    Missing {
        /// What the manifest said.
        named: String,
    },

    /// Nothing here runs files of that kind.
    #[error(
        "this application is written in a language this device cannot run (`.{extension}`). \
         It can run {known}, or an application compiled to WebAssembly."
    )]
    UnknownLanguage {
        /// The extension, without its dot.
        extension: String,
        /// What this build can run instead.
        known: String,
    },

    /// The language is known and its interpreter is not installed.
    #[error(
        "running {language} on this device needs the {language} interpreter, \
         which is not installed. Put it at {}.", expected.display()
    )]
    NoInterpreter {
        /// Which language.
        language: &'static str,
        /// Exactly where the file has to go.
        expected: PathBuf,
    },
}

impl Program {
    /// What to run for an application, or why there is nothing.
    ///
    /// `declared` is the manifest's own `program` field, `source` is the
    /// application's source directory and `interpreters` is where shared
    /// interpreters live. Nothing is guessed: an application that did not say
    /// what to run does not get a scan of its directory for something that
    /// looks executable, because "something that looks executable" in a tree a
    /// model wrote is not a safe thing to pick.
    ///
    /// # Errors
    ///
    /// [`NoProgram`], every variant of which names what is wrong and what to
    /// do about it.
    pub fn locate(
        declared: Option<&str>,
        source: &Path,
        interpreters: &Path,
    ) -> Result<Self, NoProgram> {
        let named = declared.map(str::trim).filter(|named| !named.is_empty());
        let named = named.ok_or(NoProgram::NotDeclared)?;

        // Checked before joining rather than after. A path that climbs out is
        // refused as written, so no version of this depends on a comparison
        // between two paths that may or may not have been canonicalised.
        let plain = Path::new(named);
        let escapes = plain.is_absolute()
            || plain
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)));
        if escapes {
            return Err(NoProgram::Escapes {
                named: named.to_owned(),
            });
        }

        let file = source.join(plain);
        if !file.is_file() {
            return Err(NoProgram::Missing {
                named: named.to_owned(),
            });
        }

        let extension = plain
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        if extension == "wasm" {
            return Ok(Self::module(file));
        }

        let Some((_, language, module)) = INTERPRETERS
            .iter()
            .find(|(known, _, _)| *known == extension)
        else {
            return Err(NoProgram::UnknownLanguage {
                extension,
                known: languages(),
            });
        };

        let interpreter = interpreters.join(module);
        if !interpreter.is_file() {
            return Err(NoProgram::NoInterpreter {
                language,
                expected: interpreter,
            });
        }

        Self::interpreted(interpreter, &file).ok_or(NoProgram::Escapes {
            named: named.to_owned(),
        })
    }
}

/// The languages this build can run, listed the way a sentence would.
#[must_use]
pub fn languages() -> String {
    let mut names: Vec<&str> = INTERPRETERS
        .iter()
        .map(|(_, language, _)| *language)
        .collect();
    names.sort_unstable();
    names.dedup();
    names.join(" or ")
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

    /// One application, arranged on disk.
    fn arranged() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let source = home.path().join("source");
        let interpreters = home.path().join("interpreters");
        std::fs::create_dir_all(&source).expect("a source directory");
        std::fs::create_dir_all(&interpreters).expect("an interpreter directory");
        (home, source, interpreters)
    }

    #[test]
    fn a_compiled_application_is_run_directly() {
        let (_home, source, interpreters) = arranged();
        std::fs::write(source.join("program.wasm"), b"\0asm").expect("a module");

        let program = Program::locate(Some("program.wasm"), &source, &interpreters)
            .expect("it is right there");

        assert_eq!(program, Program::module(source.join("program.wasm")));
    }

    #[test]
    fn a_script_is_matched_to_the_interpreter_for_its_language() {
        let (_home, source, interpreters) = arranged();
        std::fs::write(source.join("main.js"), "// unused").expect("a script");
        std::fs::write(interpreters.join("javascript.wasm"), b"\0asm").expect("an interpreter");

        let program =
            Program::locate(Some("main.js"), &source, &interpreters).expect("both are there");

        assert_eq!(program.wasm(), interpreters.join("javascript.wasm"));
        assert_eq!(program.leading_arguments(), vec!["/program/main.js"]);
    }

    /// **Nothing is guessed.** An application that did not say what to run does
    /// not get its directory scanned for something that looks executable — in
    /// a tree a model wrote, "looks executable" is not a safe thing to pick.
    #[test]
    fn an_application_that_says_nothing_is_not_searched_for_a_program() {
        let (_home, source, interpreters) = arranged();
        std::fs::write(source.join("main.js"), "// unused").expect("a script");
        std::fs::write(source.join("program.wasm"), b"\0asm").expect("a module");

        assert_eq!(
            Program::locate(None, &source, &interpreters),
            Err(NoProgram::NotDeclared)
        );
        assert_eq!(
            Program::locate(Some("   "), &source, &interpreters),
            Err(NoProgram::NotDeclared),
            "and a blank name is saying nothing"
        );
    }

    /// A manifest that names a path outside the application is refused as
    /// written, before anything is joined. A manifest is generated content,
    /// and this is the field through which it would choose what runs.
    #[test]
    fn a_manifest_cannot_name_a_program_outside_the_application() {
        let (home, source, interpreters) = arranged();
        std::fs::write(home.path().join("elsewhere.wasm"), b"\0asm").expect("a module");

        for climbing in [
            "../elsewhere.wasm",
            "nested/../../elsewhere.wasm",
            "/etc/passwd",
        ] {
            assert!(
                matches!(
                    Program::locate(Some(climbing), &source, &interpreters),
                    Err(NoProgram::Escapes { .. })
                ),
                "{climbing} must be refused"
            );
        }
    }

    /// A missing interpreter says which one and exactly where to put it. This
    /// is the error somebody is most likely to see, and the one where a vague
    /// message would waste the most of their time.
    #[test]
    fn a_missing_interpreter_says_which_and_where() {
        let (_home, source, interpreters) = arranged();
        std::fs::write(source.join("main.js"), "// unused").expect("a script");

        let refused = Program::locate(Some("main.js"), &source, &interpreters)
            .expect_err("the interpreter is not installed");

        let said = refused.to_string();
        assert!(said.contains("JavaScript"), "{said}");
        assert!(said.contains("javascript.wasm"), "{said}");
        assert!(
            said.contains(&interpreters.display().to_string()),
            "and where it goes: {said}"
        );
    }

    /// A language this build cannot run says so, and says what it can.
    #[test]
    fn an_unrunnable_language_says_what_this_device_can_run() {
        let (_home, source, interpreters) = arranged();
        std::fs::write(source.join("main.hs"), "-- unused").expect("a script");

        let refused = Program::locate(Some("main.hs"), &source, &interpreters)
            .expect_err("nothing here runs Haskell");

        let said = refused.to_string();
        assert!(said.contains(".hs"), "{said}");
        assert!(said.contains("JavaScript"), "{said}");
        assert!(said.contains("Python"), "{said}");
    }

    #[test]
    fn a_declared_program_that_is_not_there_says_so_by_name() {
        let (_home, source, interpreters) = arranged();

        let refused = Program::locate(Some("main.js"), &source, &interpreters)
            .expect_err("nothing was written");

        assert_eq!(
            refused,
            NoProgram::Missing {
                named: "main.js".to_owned()
            }
        );
    }

    /// Every language in the table has an interpreter file name, and the list
    /// a person is shown is built from the same table the resolver uses. Two
    /// lists would drift, and the one that drifted would be the one in the
    /// error message nobody tests.
    #[test]
    fn the_languages_offered_are_the_languages_resolved() {
        let offered = languages();

        for (_, language, module) in INTERPRETERS {
            assert!(offered.contains(language), "{language} is not offered");
            assert_eq!(
                std::path::Path::new(module).extension(),
                Some(std::ffi::OsStr::new("wasm")),
                "{module} is not a module"
            );
        }
    }

    #[test]
    fn a_script_that_cannot_be_named_is_refused_rather_than_guessed_at() {
        assert!(Program::interpreted("/interpreters/js.wasm", Path::new("/")).is_none());
    }
}
