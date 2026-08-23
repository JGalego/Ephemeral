//! The runtime a phone can have.
//!
//! [`super::run`] is the interpreter and [`Program`] is what to feed it. This
//! is the layer that turns a granted [`ContainerSpec`] into one run and reports
//! it in the same words the container runtime uses, so that nothing above the
//! runtime layer needs to know which sandbox an application happened to get.
//!
//! ## What it does not implement, and why that is not a gap
//!
//! [`crate::Runtime`] is shaped by containers: images to pull, a thing to
//! start and later stop, pause and resume, logs to tail. None of that has a
//! meaning here. A module is not a daemon — it is loaded, it runs, it is gone,
//! and there is nothing left to pause. Implementing that trait would mean six
//! methods returning "not supported", which is a worse description of this
//! runtime than not claiming the shape at all.
//!
//! What it offers instead is the one operation a module has: run it to
//! completion, under exactly what was granted, and say what happened.

use std::time::Duration;

use crate::{Completed, RuntimeError, Secrets, spec::ContainerSpec};

use super::{Capabilities, Confined, Program, WasmError, run};

/// Runs applications as WebAssembly modules, in this process.
#[derive(Debug, Clone, Copy, Default)]
pub struct WasmRuntime;

impl WasmRuntime {
    /// A runtime.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The name of this runtime, for the interface and the audit log.
    #[must_use]
    pub fn name(&self) -> &'static str {
        "WebAssembly"
    }

    /// Whether this runtime can be used.
    ///
    /// Always, and that is the point of it. The interpreter is compiled into
    /// this binary; there is no daemon to be running, no socket to connect to
    /// and nothing to install. A runtime that is *sometimes* available is what
    /// left mobile with nothing.
    #[must_use]
    pub fn availability(&self) -> crate::Availability {
        crate::Availability {
            usable: true,
            version: Some(format!("wasmi {}", env!("CARGO_PKG_VERSION"))),
            explanation: "Applications run as WebAssembly on this device, \
                          reaching only what you have allowed."
                .to_owned(),
        }
    }

    /// Runs one application to completion under exactly what `spec` granted.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CannotEnforce`] when `spec` asks for a confinement this
    /// runtime cannot express, [`RuntimeError::Ungranted`] when the module
    /// imports something it was not given, and
    /// [`RuntimeError::CommandFailed`] when the program cannot be read or
    /// loaded. An application that runs and exits non-zero, or that is stopped
    /// for exceeding a bound, is **not** an error — that is a [`Completed`]
    /// with a non-zero exit code, because a failing program is an answer.
    pub fn run_once(
        &self,
        program: &Program,
        spec: &ContainerSpec,
        timeout: Duration,
        secrets: &Secrets,
    ) -> Result<Completed, RuntimeError> {
        let capabilities = self.capabilities(program, spec, timeout, secrets)?;

        let wasm = std::fs::read(program.wasm()).map_err(|error| RuntimeError::CommandFailed {
            command: format!("run {}", program.wasm().display()),
            status: "unreadable".to_owned(),
            stderr: format!("{} could not be read: {error}", program.wasm().display()),
        })?;

        match run(&wasm, &capabilities) {
            Ok(confined) => Ok(reported(&confined)),
            Err(WasmError::Ungranted(said)) => Err(RuntimeError::Ungranted {
                app: spec.app.clone(),
                reason: said,
            }),
            Err(error) => Err(RuntimeError::CommandFailed {
                command: format!("run {}", program.wasm().display()),
                status: "did not start".to_owned(),
                stderr: error.to_string(),
            }),
        }
    }

    /// What the module will be given, or a refusal saying what could not be.
    ///
    /// Separate from running so that the translation can be asserted about
    /// without executing anything — it is the whole security argument, and a
    /// security argument that can only be checked by running code is one that
    /// is checked less often.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::CannotEnforce`] for anything `spec` grants that this
    /// runtime cannot give effect to. Refusing is the rule: an application that
    /// was promised the network and silently did not get it would look broken
    /// in a way nothing explains.
    pub fn capabilities(
        &self,
        program: &Program,
        spec: &ContainerSpec,
        timeout: Duration,
        secrets: &Secrets,
    ) -> Result<Capabilities, RuntimeError> {
        // The one place this runtime is *stricter* than the container one.
        // There are no sockets in this WASI implementation to hand out, so an
        // egress grant cannot be honoured — and appearing to honour it is worse
        // than saying so, because the application would fail at its first
        // request with a message about a network that was never there.
        if spec.egress.is_permitted() {
            return Err(RuntimeError::CannotEnforce {
                control: "network access".to_owned(),
                reason: "WebAssembly applications on this device have no network at all, \
                         so an application that needs one cannot run here. On a desktop \
                         with Docker it can."
                    .to_owned(),
            });
        }

        let mut capabilities = Capabilities::from_spec(spec, timeout, secrets);

        if let Some(mounted) = program.mounted() {
            capabilities.visible.push(mounted);
        }

        let mut arguments = program.leading_arguments();
        arguments.append(&mut capabilities.arguments);
        capabilities.arguments = arguments;

        Ok(capabilities)
    }
}

/// One run's outcome, in the words the container runtime would have used.
fn reported(confined: &Confined) -> Completed {
    let mut output = confined.output.clone();

    if !confined.diagnostics.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&confined.diagnostics);
    }

    // Said by Ephemeral rather than by the application, and appended rather
    // than substituted: what a program managed to print before it was stopped
    // is often the only thing that explains where it got to.
    if let Some(halted) = confined.halted {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(halted.explain());
        output.push('\n');
    }

    Completed {
        succeeded: confined.succeeded(),
        exit_code: confined.exit_code,
        output,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io::Write as _;

    use ephemeral_core::AppId;

    use super::*;
    use crate::spec::{Egress, Mount};
    use crate::wasm::Halt;

    fn app() -> AppId {
        AppId::parse("tally").expect("a valid id")
    }

    fn spec() -> ContainerSpec {
        ContainerSpec::minimal(app(), "unused", Vec::new())
    }

    /// Writes a module compiled from WebAssembly text, and returns its path.
    fn compiled(into: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
        let path = into.join(name);
        let bytes = wat::parse_str(text).expect("the test module should assemble");
        std::fs::File::create(&path)
            .expect("a writable temporary directory")
            .write_all(&bytes)
            .expect("the module should be written");
        path
    }

    /// Prints its own arguments, one per line, skipping argument zero.
    ///
    /// The smallest program that can prove an argument vector arrived intact,
    /// written in text so that what it does is reviewable.
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
        ;; Find the null terminator of the string at $pointer.
        (local.set $end (local.get $pointer))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (local.get $end))))
            (local.set $end (i32.add (local.get $end) (i32.const 1)))
            (br $scan)))
        ;; Overwrite it with a newline, so one buffer holds the whole line.
        (i32.store8 (local.get $end) (i32.const 10))
        (i32.store (i32.const 8) (local.get $pointer))
        (i32.store (i32.const 12)
          (i32.add (i32.sub (local.get $end) (local.get $pointer)) (i32.const 1)))
        (drop (call $write (i32.const 1) (i32.const 8) (i32.const 1) (i32.const 24))))
      (func (export "_start")
        (drop (call $sizes (i32.const 64) (i32.const 68)))
        (global.set $count (i32.load (i32.const 64)))
        (drop (call $args (i32.const 1024) (i32.const 2048)))
        ;; Skip argument zero, which is the program's own name.
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

    /// A module runs, and its exit code and output come back in the same shape
    /// the container runtime uses.
    #[test]
    fn a_module_runs_and_reports_itself_like_a_container_would() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let module = compiled(
            home.path(),
            "program.wasm",
            r#"(module (func (export "_start")))"#,
        );

        let completed = WasmRuntime::new()
            .run_once(
                &Program::module(module),
                &spec(),
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect("it runs");

        assert!(completed.succeeded);
        assert_eq!(completed.exit_code, 0);
    }

    /// **The interpreted path, end to end.**
    ///
    /// A phone cannot compile anything, so the only way it runs a program
    /// written seconds ago is for the program to be a file and the interpreter
    /// to be the module. This asserts the two halves meet: the interpreter is
    /// what loads, and it is told where the script is in its own terms.
    #[test]
    fn an_interpreted_program_is_handed_its_script() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let source = home.path().join("source");
        std::fs::create_dir(&source).expect("a source directory");
        std::fs::write(source.join("main.js"), "console.log('hello')").expect("a script");

        let interpreter = compiled(home.path(), "js.wasm", ECHOES_ITS_ARGUMENTS);
        let program = Program::interpreted(interpreter, &source.join("main.js"))
            .expect("a named script in a directory");

        let completed = WasmRuntime::new()
            .run_once(&program, &spec(), Duration::from_secs(30), &Secrets::new())
            .expect("it runs");

        assert!(completed.succeeded, "{}", completed.output);
        assert_eq!(
            completed.output, "/program/main.js\n",
            "the interpreter is told where the script is, in its own view of the world"
        );
    }

    /// The application's own arguments come after the interpreter's, in order.
    /// Getting this wrong would hand a form's answers to the interpreter and
    /// the script's path to the application.
    #[test]
    fn the_applications_arguments_follow_the_interpreters() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let source = home.path().join("source");
        std::fs::create_dir(&source).expect("a source directory");
        std::fs::write(source.join("main.js"), "// unused").expect("a script");

        let interpreter = compiled(home.path(), "js.wasm", ECHOES_ITS_ARGUMENTS);
        let program = Program::interpreted(interpreter, &source.join("main.js"))
            .expect("a named script in a directory");

        let mut spec = spec();
        spec.entrypoint = vec!["--count".to_owned(), "rows".to_owned()];

        let completed = WasmRuntime::new()
            .run_once(&program, &spec, Duration::from_secs(30), &Secrets::new())
            .expect("it runs");

        assert_eq!(completed.output, "/program/main.js\n--count\nrows\n");
    }

    /// A script's directory is visible so a program can reach its own files,
    /// and read-only so it cannot rewrite itself between runs.
    #[test]
    fn a_script_is_visible_to_its_interpreter_and_nothing_else_is() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let source = home.path().join("source");
        std::fs::create_dir(&source).expect("a source directory");
        std::fs::write(source.join("main.js"), "// unused").expect("a script");

        let program = Program::interpreted(home.path().join("js.wasm"), &source.join("main.js"))
            .expect("a named script in a directory");

        let capabilities = WasmRuntime::new()
            .capabilities(&program, &spec(), Duration::from_secs(30), &Secrets::new())
            .expect("nothing here needs refusing");

        assert_eq!(
            capabilities.visible,
            vec![(source, "/program".to_owned())],
            "its own source, and nothing of the user's"
        );
        assert!(
            !capabilities.writable,
            "an interpreter that could rewrite its script is an application that edits itself"
        );
    }

    /// A grant the container runtime honours and this one cannot is refused
    /// rather than quietly dropped. An application promised the network and
    /// silently denied it looks broken in a way nothing explains.
    #[test]
    fn an_application_that_needs_the_network_is_refused_here_and_told_why() {
        let mut spec = spec();
        spec.egress = Egress::AllowList(vec![
            ephemeral_core::permission::HostScope::parse("api.example.com").expect("a host"),
        ]);

        let refused = WasmRuntime::new()
            .capabilities(
                &Program::module("unused.wasm"),
                &spec,
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect_err("this runtime has no network to give");

        let said = refused.to_string();
        assert!(said.contains("network"), "{said}");
        assert!(
            said.contains("Docker"),
            "and it says where the application can run instead: {said}"
        );
    }

    /// A module importing something nothing provides is a refusal naming the
    /// application, not a mysterious runtime failure.
    #[test]
    fn a_module_asking_for_more_than_it_was_given_is_refused_by_name() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let module = compiled(
            home.path(),
            "program.wasm",
            r#"(module
                 (import "host" "exfiltrate" (func $out (param i32)))
                 (func (export "_start") (call $out (i32.const 1))))"#,
        );

        let refused = WasmRuntime::new()
            .run_once(
                &Program::module(module),
                &spec(),
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect_err("it must not be allowed to start");

        assert!(
            matches!(refused, RuntimeError::Ungranted { .. }),
            "expected a refusal naming the application, got {refused}"
        );
        assert!(refused.to_string().contains("tally"));
    }

    /// A program that was stopped reports the exit code a container would have
    /// reported, and says in words what happened.
    ///
    /// A crash rather than a runaway loop, because exhausting a whole second of
    /// fuel takes an interpreter a whole second and this assertion is about the
    /// reporting rather than about the bound. What being stopped for processing
    /// looks like is asserted in [`super::tests`], where the allowance can be
    /// small, and how it is reported is asserted just below.
    #[test]
    fn a_stopped_program_reports_what_a_container_would_have() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let module = compiled(
            home.path(),
            "program.wasm",
            r#"(module (func (export "_start") (unreachable)))"#,
        );

        let completed = WasmRuntime::new()
            .run_once(
                &Program::module(module),
                &spec(),
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect("being stopped is an outcome, not a failure to run");

        assert!(!completed.succeeded);
        assert_eq!(completed.exit_code, 134);
        assert!(completed.output.contains("Nothing was left running"));
    }

    /// What a program printed before it was stopped survives into the report,
    /// with Ephemeral's explanation added after it rather than in place of it.
    /// Substituting would discard the only thing that says where it got to.
    #[test]
    fn being_stopped_adds_an_explanation_without_replacing_the_output() {
        let reported = reported(&Confined {
            exit_code: Halt::Processing.exit_code(),
            output: "counted 4 rows".to_owned(),
            diagnostics: String::new(),
            halted: Some(Halt::Processing),
        });

        assert!(!reported.succeeded);
        assert_eq!(reported.exit_code, 124, "what `timeout` exits with");
        assert!(reported.output.starts_with("counted 4 rows\n"));
        assert!(
            reported
                .output
                .contains("more processing than it was allowed")
        );
    }

    /// Both streams reach the caller. A program that explained its failure on
    /// standard error and printed nothing else would otherwise look silent.
    #[test]
    fn what_went_to_standard_error_is_not_lost() {
        let reported = reported(&Confined {
            exit_code: 1,
            output: String::new(),
            diagnostics: "no such column: total".to_owned(),
            halted: None,
        });

        assert!(!reported.succeeded);
        assert_eq!(reported.output, "no such column: total");
    }

    /// A granted folder reaches the module as a directory it can name.
    #[test]
    fn a_granted_folder_reaches_the_module() {
        let home = tempfile::tempdir().expect("a temporary directory");

        let mut spec = spec();
        spec.mounts.push(Mount::read_only(home.path(), "/mnt/data"));

        let capabilities = WasmRuntime::new()
            .capabilities(
                &Program::module("unused.wasm"),
                &spec,
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect("nothing here needs refusing");

        assert_eq!(
            capabilities.visible,
            vec![(home.path().to_path_buf(), "/mnt/data".to_owned())]
        );
    }

    /// Reading the program is a failure with a path in it, not a panic.
    #[test]
    fn a_program_that_is_not_there_says_which_one() {
        let refused = WasmRuntime::new()
            .run_once(
                &Program::module("/nowhere/at/all/program.wasm"),
                &spec(),
                Duration::from_secs(30),
                &Secrets::new(),
            )
            .expect_err("there is nothing to run");

        assert!(refused.to_string().contains("program.wasm"), "{refused}");
    }

    /// It is available wherever this binary is, which is the whole point.
    #[test]
    fn this_runtime_is_always_available() {
        assert!(WasmRuntime::new().availability().usable);
    }
}
