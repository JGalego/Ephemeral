//! One run, end to end, in the one place both clients call.
//!
//! [`super::run`] is the interpreter, [`Program`] is what to feed it and
//! [`WasmRuntime`] is one confined execution. This is the sequence that turns
//! *an application somebody has* into *a thing that ran*: what the manifest
//! says it is, what a person granted it, what that grant becomes as a sandbox,
//! what there is to run, and the run.
//!
//! It lives here rather than in a client because there is more than one client
//! and there must not be more than one of this. A phone and a terminal each
//! composing the same five steps are two subtly different Ephemerals, and the
//! step one of them gets wrong is the one nobody compares.
//!
//! What it deliberately does **not** do is read the ledger. Deciding what an
//! application was granted is the domain's work and this crate does not depend
//! on the crate that does it; callers pass the answer in. Passing the manifest
//! instead would be the bug this whole design exists to prevent — a manifest is
//! what an application *asked* for.

use std::path::PathBuf;
use std::time::Duration;

use ephemeral_core::{
    AppManifest,
    manifest::{AppInterface, RuntimeKind},
    permission::AppPermission,
    storage::StorageLayout,
};

use crate::{Completed, HostPaths, RuntimeError, Secrets, spec::ContainerSpec};

use super::{Program, WasmRuntime};

/// An application, and everything needed to run it once.
#[derive(Debug)]
pub struct Runnable<'a> {
    /// What the application is.
    pub manifest: &'a AppManifest,

    /// Where everything lives on this machine.
    pub layout: &'a StorageLayout,

    /// What a **person** allowed it, from the ledger.
    ///
    /// Never derived from `manifest`. Building a sandbox from what an
    /// application requested would let it widen its own confinement by asking.
    pub granted: &'a [AppPermission],

    /// What `~` means in a permission scope on this machine.
    ///
    /// A parameter rather than a lookup, because the honest answer differs: a
    /// desktop has a home directory, and a handset has Ephemeral's own root and
    /// nothing else that `~` could truthfully mean.
    pub home: PathBuf,

    /// What somebody asked for, composed by the domain from a form.
    ///
    /// **Added to what the application declares, never replacing it.** An
    /// application's entry point is part of what it *is*, recorded in its
    /// version, and a command line that could replace it would let somebody run
    /// something other than the application they are looking at. The container
    /// runtime has always held that; this one dropped the declaration entirely
    /// until a review noticed.
    pub arguments: Vec<String>,

    /// The longest it may run, whatever its manifest declares.
    ///
    /// [`super::HANDHELD_CEILING`] where somebody is waiting for it.
    pub ceiling: Duration,
}

/// What one run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// How it exited, and what it printed.
    pub completed: Completed,

    /// How the output is meant to be shown: [`Shown::Page`] or [`Shown::Text`].
    pub shown: Shown,

    /// Access that was granted and will not be given effect, with the reason.
    ///
    /// Carried rather than dropped. Somebody who allowed an application to read
    /// a folder and watches it fail to find the folder is owed the reason, and
    /// the reason is us.
    pub refused: Vec<String>,
}

/// How an application's output is meant to be presented.
///
/// Decided once, here, from what the application declared itself to be — never
/// by a client guessing from the shape of the bytes. A client that sniffed for
/// a leading `<` would render a comparison's first line of markup as a document
/// and a document beginning with a newline as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shown {
    /// Rendered. The application wrote a page.
    Page,

    /// Read. Everything else.
    Text,
}

impl Shown {
    /// The name a client sees across a boundary.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Text => "text",
        }
    }

    /// How an application with this interface presents its output.
    ///
    /// A WebAssembly application has no socket and cannot be a server, so "a
    /// web application" here is one that *writes* a page for the host to
    /// render. That is not a lesser version of the idea — it is why showing
    /// somebody a user interface costs no network permission at all.
    #[must_use]
    pub fn of(interface: AppInterface) -> Self {
        match interface {
            AppInterface::Web => Self::Page,
            _ => Self::Text,
        }
    }
}

/// Runs one application under exactly what it was granted.
///
/// # Errors
///
/// [`RuntimeError::CannotEnforce`] when the application is not one this runtime
/// can run, or asks for a confinement it cannot express;
/// [`RuntimeError::Ungranted`] when its module imports something it was not
/// given; [`RuntimeError::CommandFailed`] when there is nothing to run or it
/// cannot be read. An application that runs and *fails* is not an error — that
/// is a [`Ran`] with a non-zero exit code, because a failing program is an
/// answer.
pub fn run(runnable: &Runnable<'_>) -> Result<Ran, RuntimeError> {
    let app = &runnable.manifest.id;

    let runtime =
        runnable
            .manifest
            .runtime
            .as_ref()
            .ok_or_else(|| RuntimeError::CannotEnforce {
                control: format!("running {app}"),
                reason: format!(
                    "{app} has not been generated yet, so there is nothing to run. \
                 Describe it and generate it first."
                ),
            })?;

    if runtime.kind != RuntimeKind::Wasm {
        return Err(RuntimeError::CannotEnforce {
            control: format!("running {app} here"),
            reason: format!(
                "{app} runs on {}, and this is the WebAssembly runtime. \
                 A {} application needs a computer with Docker.",
                runtime.kind, runtime.kind
            ),
        });
    }

    let paths = HostPaths {
        home: runnable.home.clone(),
        // Its own storage, which every application has and nothing has to
        // grant. Created rather than assumed: an application generated before
        // this directory was part of the layout, or restored from a recipe, has
        // never had one, and "no such file or directory" is a terrible way to
        // find that out.
        data_dir: prepared(runnable.layout, runnable.manifest)?,
    };

    // What the application declares, then what somebody asked for. In that
    // order, and both: see [`Runnable::arguments`].
    let mut arguments = runtime.entrypoint.clone();
    arguments.extend(runnable.arguments.iter().cloned());

    let spec = ContainerSpec::from_grants(
        app.clone(),
        // No image. This runtime has none; the field belongs to the one that
        // does.
        String::new(),
        arguments,
        runnable.manifest.resources,
        runnable.granted,
        &paths,
    )?;

    let refused = spec
        .refused
        .iter()
        .map(|refusal| format!("Not granting {} — {}", refusal.granted, refusal.reason))
        .collect();

    let program = Program::locate(
        runtime.program.as_deref(),
        &runnable.layout.app(app).source(),
        &runnable.layout.interpreters_dir(),
    )
    .map_err(|error| RuntimeError::CommandFailed {
        command: format!("run {app}"),
        status: "nothing to run".to_owned(),
        stderr: error.to_string(),
    })?;

    let completed = WasmRuntime::new().run_once(
        &program,
        &spec,
        allowance(runnable.manifest, runnable.ceiling),
        &Secrets::new(),
    )?;

    Ok(Ran {
        completed,
        shown: Shown::of(runtime.interface),
        refused,
    })
}

/// The application's own storage, made sure to exist.
fn prepared(layout: &StorageLayout, manifest: &AppManifest) -> Result<PathBuf, RuntimeError> {
    let data = layout.app(&manifest.id).data();

    std::fs::create_dir_all(&data).map_err(|error| RuntimeError::CannotEnforce {
        control: format!("{}'s own storage", manifest.id),
        reason: format!("{} could not be created: {error}", data.display()),
    })?;

    Ok(data)
}

/// How long this application may run, given a ceiling.
///
/// The smaller of what the manifest declares and what the caller will wait for.
/// A manifest written for a desktop may say fifteen minutes, which on a phone is
/// not a long job but a frozen application.
fn allowance(manifest: &AppManifest, ceiling: Duration) -> Duration {
    let declared = manifest.resources.max_runtime.map_or(ceiling, |period| {
        // Zero or negative is not a shorter allowance, it is a nonsense one,
        // and the safe reading of nonsense is the ceiling rather than nothing.
        u64::try_from(period.as_seconds()).map_or(ceiling, Duration::from_secs)
    });

    declared.min(ceiling)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use ephemeral_core::{AppId, manifest::RuntimeSpec, retention::RetentionPeriod};

    use super::super::HANDHELD_CEILING;
    use super::*;

    fn app() -> AppId {
        AppId::parse("tally").expect("a valid id")
    }

    fn wasm_manifest() -> AppManifest {
        let mut manifest = AppManifest::requested(app(), "Tally");
        manifest.runtime = Some(RuntimeSpec::wasm_job("program.wasm", Vec::new()));
        manifest
    }

    /// One application on disk, and a layout pointing at it.
    fn installed(home: &std::path::Path, text: &str) -> StorageLayout {
        let layout = StorageLayout::new(home);
        let source = layout.app(&app()).source();
        std::fs::create_dir_all(&source).expect("a source directory");
        std::fs::write(
            source.join("program.wasm"),
            wat::parse_str(text).expect("the test application should assemble"),
        )
        .expect("the module should be written");
        layout
    }

    const SAYS_SOMETHING: &str = r#"(module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 32) "4 rows differ")
      (func (export "_start")
        (i32.store (i32.const 0) (i32.const 32))
        (i32.store (i32.const 4) (i32.const 13))
        (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 16)))))"#;

    #[test]
    fn an_application_runs_and_says_what_it_did() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = installed(home.path(), SAYS_SOMETHING);
        let manifest = wasm_manifest();

        let ran = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: Vec::new(),
            ceiling: HANDHELD_CEILING,
        })
        .expect("it runs");

        assert!(ran.completed.succeeded);
        assert_eq!(ran.completed.output, "4 rows differ");
        assert_eq!(ran.shown, Shown::Text);
    }

    /// Prints its own arguments, one per line, skipping argument zero.
    const ECHOES_ITS_ARGUMENTS: &str = r#"(module
      (import "wasi_snapshot_preview1" "args_sizes_get"
        (func $sizes (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "args_get"
        (func $args (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 2)
      (global $count (mut i32) (i32.const 0))
      (global $at (mut i32) (i32.const 0))
      (func $print_at (param $pointer i32)
        (local $end i32)
        (local.set $end (local.get $pointer))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (local.get $end))))
            (local.set $end (i32.add (local.get $end) (i32.const 1)))
            (br $scan)))
        (i32.store8 (local.get $end) (i32.const 10))
        (i32.store (i32.const 8) (local.get $pointer))
        (i32.store (i32.const 12)
          (i32.add (i32.sub (local.get $end) (local.get $pointer)) (i32.const 1)))
        (drop (call $write (i32.const 1) (i32.const 8) (i32.const 1) (i32.const 24))))
      (func (export "_start")
        (drop (call $sizes (i32.const 64) (i32.const 68)))
        (global.set $count (i32.load (i32.const 64)))
        (drop (call $args (i32.const 1024) (i32.const 2048)))
        (global.set $at (i32.const 1))
        (block $done
          (loop $next
            (br_if $done (i32.ge_u (global.get $at) (global.get $count)))
            (call $print_at
              (i32.load (i32.add (i32.const 1024)
                                 (i32.mul (global.get $at) (i32.const 4)))))
            (global.set $at (i32.add (global.get $at) (i32.const 1)))
            (br $next))))
    )"#;

    /// **An application's entry point is part of what it is.**
    ///
    /// `start` has always said so for containers: arguments are *added* to the
    /// declaration, never substituted for it, because a command line that could
    /// replace it would let somebody run something other than the application
    /// they are looking at. This runtime dropped the declaration entirely until
    /// a review noticed the two paths disagreed.
    #[test]
    fn what_somebody_typed_is_added_to_what_the_application_declares() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = installed(home.path(), ECHOES_ITS_ARGUMENTS);

        let mut manifest = AppManifest::requested(app(), "Tally");
        manifest.runtime = Some(RuntimeSpec::wasm_job(
            "program.wasm",
            vec!["--format".to_owned(), "csv".to_owned()],
        ));

        let ran = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: vec!["--verbose".to_owned()],
            ceiling: HANDHELD_CEILING,
        })
        .expect("it runs");

        assert_eq!(
            ran.completed.output, "--format\ncsv\n--verbose\n",
            "the declaration comes first and survives; what was typed follows it"
        );
    }

    /// **A granted directory is the only one it can reach, symlinks included.**
    ///
    /// The escape a preopened directory has to withstand: a link inside the
    /// application's own source pointing somewhere it was never granted. This
    /// is cap-std's job rather than Ephemeral's, which is exactly why it is
    /// worth an assertion — a property nothing checks is a property that is
    /// true until a dependency changes.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_a_granted_directory_leads_nowhere() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let outside = home.path().join("not-granted");
        std::fs::create_dir_all(&outside).expect("somewhere it was not given");
        std::fs::write(outside.join("secret"), "the user's private notes").expect("a file");

        let layout = installed(home.path(), READS_A_FILE);
        std::os::unix::fs::symlink(&outside, layout.app(&app()).source().join("escape"))
            .expect("a link pointing out of the application");

        let mut manifest = AppManifest::requested(app(), "Tally");
        // Interpreted, because that is the shape that preopens the application's
        // own source — the one directory a generated tree could plant a link in.
        manifest.runtime = Some(RuntimeSpec::wasm_job("main.js", Vec::new()));
        std::fs::write(layout.app(&app()).source().join("main.js"), "// unused").expect("a script");
        let interpreters = layout.interpreters_dir();
        std::fs::create_dir_all(&interpreters).expect("an interpreter directory");
        std::fs::copy(
            layout.app(&app()).source().join("program.wasm"),
            interpreters.join("javascript.wasm"),
        )
        .expect("an interpreter");

        let ran = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            // Relative to the preopened directory, which is how a WASI path
            // is named: the module holds a descriptor, not a filesystem.
            arguments: vec!["escape/secret".to_owned()],
            ceiling: HANDHELD_CEILING,
        })
        .expect("it runs, and finds nothing");

        assert!(
            !ran.completed.output.contains("private notes"),
            "it read through a link out of what it was granted: {}",
            ran.completed.output
        );
    }

    /// The control for the test above. A module that cannot read anything would
    /// pass it without the sandbox doing a thing, so this proves the same
    /// module *does* read a file when the file is one it was given.
    #[cfg(unix)]
    #[test]
    fn the_same_module_reads_a_file_it_was_given() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = installed(home.path(), READS_A_FILE);
        let source = layout.app(&app()).source();
        std::fs::write(source.join("notes"), "the user's private notes").expect("a file");
        std::fs::write(source.join("main.js"), "// unused").expect("a script");

        let interpreters = layout.interpreters_dir();
        std::fs::create_dir_all(&interpreters).expect("an interpreter directory");
        std::fs::copy(
            source.join("program.wasm"),
            interpreters.join("javascript.wasm"),
        )
        .expect("an interpreter");

        let mut manifest = AppManifest::requested(app(), "Tally");
        manifest.runtime = Some(RuntimeSpec::wasm_job("main.js", Vec::new()));

        let ran = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: vec!["notes".to_owned()],
            ceiling: HANDHELD_CEILING,
        })
        .expect("it runs");

        assert!(
            ran.completed.output.contains("private notes"),
            "the module cannot read anything at all, so the escape test proves nothing: {:?}",
            ran.completed
        );
    }

    /// Opens the path in argument one and prints what is in it, or nothing.
    const READS_A_FILE: &str = r#"(module
      (import "wasi_snapshot_preview1" "args_sizes_get"
        (func $sizes (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "args_get"
        (func $args (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "path_open"
        (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "fd_read"
        (func $read (param i32 i32 i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 4)
      (func $length (param $pointer i32) (result i32)
        (local $end i32)
        (local.set $end (local.get $pointer))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (local.get $end))))
            (local.set $end (i32.add (local.get $end) (i32.const 1)))
            (br $scan)))
        (i32.sub (local.get $end) (local.get $pointer)))
      (func (export "_start")
        (local $path i32)
        (local $opened i32)
        (drop (call $sizes (i32.const 64) (i32.const 68)))
        (drop (call $args (i32.const 1024) (i32.const 2048)))
        ;; The *last* argument. An interpreted program is handed its script
        ;; first, so argument one is not always the one a test meant.
        (if (i32.lt_u (i32.load (i32.const 64)) (i32.const 2)) (then (return)))
        (local.set $path
          (i32.load (i32.add (i32.const 1024)
                             (i32.mul (i32.sub (i32.load (i32.const 64)) (i32.const 1))
                                      (i32.const 4)))))
        ;; Preopened directory 3 is the first the host handed over.
        ;; Descriptor 4, not 3. Every application is given its own storage at
        ;; /data whether it asked or not, so that is the first preopen; the
        ;; script's directory is the second. A real interpreter never counts —
        ;; libc resolves a path against the preopen whose name it matches — but
        ;; this module speaks WASI directly and has to know.
        (if (i32.eqz (call $open
              (i32.const 4) (i32.const 1)
              (local.get $path) (call $length (local.get $path))
              (i32.const 0) (i64.const 2) (i64.const 2) (i32.const 0)
              (i32.const 80)))
          (then
            (local.set $opened (i32.load (i32.const 80)))
            (i32.store (i32.const 96) (i32.const 4096))
            (i32.store (i32.const 100) (i32.const 1024))
            (drop (call $read (local.get $opened)
                    (i32.const 96) (i32.const 1) (i32.const 104)))
            (i32.store (i32.const 112) (i32.const 4096))
            (i32.store (i32.const 116) (i32.load (i32.const 104)))
            (drop (call $write (i32.const 1)
                    (i32.const 112) (i32.const 1) (i32.const 120)))))))"#;

    /// Its own storage exists by the time it runs, even for an application that
    /// never had a directory made for it.
    #[test]
    fn an_application_that_never_had_storage_gets_some() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = installed(home.path(), SAYS_SOMETHING);
        let manifest = wasm_manifest();

        assert!(!layout.app(&app()).data().exists());

        run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: Vec::new(),
            ceiling: HANDHELD_CEILING,
        })
        .expect("it runs");

        assert!(layout.app(&app()).data().is_dir());
    }

    /// An application that has not been generated is refused with what to do
    /// about it, rather than with what went wrong.
    #[test]
    fn an_application_with_no_runtime_is_told_to_generate_first() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = StorageLayout::new(home.path());
        let manifest = AppManifest::requested(app(), "Tally");

        let refused = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: Vec::new(),
            ceiling: HANDHELD_CEILING,
        })
        .expect_err("there is nothing to run");

        assert!(refused.to_string().contains("generate"), "{refused}");
    }

    /// A container application is refused here, and told where it can run.
    #[test]
    fn a_container_application_is_refused_and_told_where_to_go() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let layout = StorageLayout::new(home.path());
        let mut manifest = AppManifest::requested(app(), "Tally");
        manifest.runtime = Some(RuntimeSpec::docker_job("python:3.12-slim", Vec::new()));

        let refused = run(&Runnable {
            manifest: &manifest,
            layout: &layout,
            granted: &[],
            home: home.path().to_path_buf(),
            arguments: Vec::new(),
            ceiling: HANDHELD_CEILING,
        })
        .expect_err("this runtime does not run containers");

        assert!(refused.to_string().contains("Docker"), "{refused}");
    }

    /// A manifest written for a desktop does not get a desktop's patience.
    #[test]
    fn a_long_declared_runtime_is_cut_to_the_ceiling() {
        let mut manifest = wasm_manifest();
        manifest.resources.max_runtime = Some(RetentionPeriod::seconds(900));

        assert_eq!(allowance(&manifest, HANDHELD_CEILING), HANDHELD_CEILING);
    }

    /// And an application that asked for less keeps the smaller number. A
    /// ceiling is not a target.
    #[test]
    fn an_application_that_asked_for_less_is_given_less() {
        let mut manifest = wasm_manifest();
        manifest.resources.max_runtime = Some(RetentionPeriod::seconds(5));

        assert_eq!(
            allowance(&manifest, HANDHELD_CEILING),
            Duration::from_secs(5)
        );
    }

    /// A nonsense allowance reads as the ceiling rather than as nothing. An
    /// application given zero seconds is one that cannot run at all, and a
    /// manifest arriving with a negative number is a bug somewhere else.
    #[test]
    fn a_nonsense_allowance_reads_as_the_ceiling() {
        let mut manifest = wasm_manifest();
        manifest.resources.max_runtime = Some(RetentionPeriod::seconds(-5));

        assert_eq!(allowance(&manifest, HANDHELD_CEILING), HANDHELD_CEILING);
    }

    #[test]
    fn a_web_application_writes_a_page_and_everything_else_writes_text() {
        assert_eq!(Shown::of(AppInterface::Web), Shown::Page);
        assert_eq!(Shown::Page.as_str(), "page");

        for interface in [
            AppInterface::CommandLine,
            AppInterface::Job,
            AppInterface::Worker,
            AppInterface::Api,
        ] {
            assert_eq!(Shown::of(interface), Shown::Text, "{interface}");
        }
    }
}
