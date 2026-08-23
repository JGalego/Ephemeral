//! The interpreter, and the four bounds it enforces.
//!
//! Everything here exists to make one sentence true: *a generated application
//! gets what it was granted and nothing else.* The four ways that could fail
//! are the four things this module bounds — reaching outside its directories,
//! reaching the network, running forever, and allocating without end. Each has
//! a test that tries it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use wasmi::{Config, Engine, Linker, Module, Store};
use wasmi_wasi::WasiCtx;

use super::Capabilities;

/// What a module did, and what it said while doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confined {
    /// Whether it finished on its own terms.
    pub succeeded: bool,

    /// What it wrote to standard output.
    pub output: String,

    /// What it wrote to standard error.
    pub diagnostics: String,
}

/// Why a module could not be run, or could not be allowed to continue.
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    /// The bytes are not a WebAssembly module this engine can load.
    #[error("that is not a WebAssembly module this build can run: {0}")]
    NotAModule(String),

    /// The module asked the host for something it was not given.
    ///
    /// The capability model working, and the most important error here: a
    /// module that imports a function nothing provides cannot start at all, so
    /// an application that wants a socket fails at the door rather than
    /// halfway through doing something.
    #[error("the application asked for something it was not granted, and will not be started: {0}")]
    Ungranted(String),

    /// It ran, and was stopped for exceeding a bound.
    #[error("the application was stopped: {0}")]
    Stopped(String),

    /// The host could not set something up.
    #[error("the sandbox could not be prepared: {0}")]
    CannotPrepare(String),
}

/// Runs one module under exactly `capabilities`, and no more.
///
/// # Errors
///
/// [`WasmError::Ungranted`] when the module imports something it was not given,
/// [`WasmError::Stopped`] when it exceeds a bound, and
/// [`WasmError::NotAModule`] when the bytes do not load. A module that runs and
/// exits non-zero is **not** an error — that is a [`Confined`] with `succeeded`
/// false, for the same reason a failing test is an answer rather than a fault.
pub fn run(wasm: &[u8], capabilities: &Capabilities) -> Result<Confined, WasmError> {
    let mut config = Config::default();
    // Metered from the start. An unmetered store runs until the process is
    // killed, and "the phone stopped responding" is not a bound.
    config.compilation_mode(wasmi::CompilationMode::Lazy);
    config.consume_fuel(true);

    let engine = Engine::new(&config);
    let module =
        Module::new(&engine, wasm).map_err(|error| WasmError::NotAModule(error.to_string()))?;

    let captured = Captured::new();
    let context = build_context(capabilities, &captured)?;

    let mut store = Store::new(&engine, context);
    store
        .set_fuel(capabilities.fuel)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let mut linker = <Linker<WasiCtx>>::new(&engine);
    // The *only* imports this module will be able to resolve. Nothing else is
    // added, so anything the application asks for beyond WASI — a socket, a
    // host function somebody invented — has nothing to bind to and the
    // instantiation below fails.
    wasmi_wasi::add_to_linker(&mut linker, |context| context)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let instance = linker
        .instantiate(&mut store, &module)
        .and_then(|pre| pre.start(&mut store))
        .map_err(|error| classify(&error))?;

    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|_| {
            WasmError::NotAModule(
                "it has no `_start`, so it is a library rather than a program".to_owned(),
            )
        })?;

    let outcome = start.call(&mut store, ());
    let (output, diagnostics) = captured.take();

    match outcome {
        Ok(()) => Ok(Confined {
            succeeded: true,
            output,
            diagnostics,
        }),
        Err(error) => {
            // An exit code is how a WASI program says it finished, well or
            // badly. That is a result rather than a malfunction, and treating
            // it as an error would report every failing test as a broken
            // sandbox.
            if let Some(status) = error.i32_exit_status() {
                return Ok(Confined {
                    succeeded: status == 0,
                    output,
                    diagnostics,
                });
            }
            Err(classify(&error))
        }
    }
}

/// Whether a failure was the sandbox holding or the module misbehaving.
///
/// Separated because the two need different words: one is Ephemeral refusing,
/// and the other is an application that did something wrong. Presenting a
/// refusal as a crash would teach somebody to distrust the sandbox.
fn classify(error: &wasmi::Error) -> WasmError {
    let said = error.to_string();

    if said.contains("unknown import") || said.contains("cannot find") {
        return WasmError::Ungranted(said);
    }
    if said.contains("fuel") || said.contains("out of fuel") {
        return WasmError::Stopped(
            "it used more processing than it was allowed. Nothing was left running.".to_owned(),
        );
    }
    if said.contains("memory") && said.contains("limit") {
        return WasmError::Stopped(
            "it asked for more memory than it was allowed. Nothing was left running.".to_owned(),
        );
    }

    WasmError::Stopped(said)
}

/// The WASI context, holding exactly what was granted.
fn build_context(capabilities: &Capabilities, captured: &Captured) -> Result<WasiCtx, WasmError> {
    let mut builder = wasmi_wasi::WasiCtxBuilder::new();

    // Argument zero is the program's own name, as every convention expects.
    let mut arguments = vec!["app".to_owned()];
    arguments.extend(capabilities.arguments.iter().cloned());
    builder
        .args(&arguments)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    for (name, value) in &capabilities.environment {
        builder
            .env(name, value)
            .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;
    }

    builder
        .stdout(Box::new(wasmi_wasi::wasi_common::pipe::WritePipe::new(
            captured.out.clone(),
        )))
        .stderr(Box::new(wasmi_wasi::wasi_common::pipe::WritePipe::new(
            captured.err.clone(),
        )));

    for (host, seen_as) in &capabilities.visible {
        let opened = open_directory(host)?;
        builder
            .preopened_dir(opened, seen_as)
            .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;
    }

    Ok(builder.build())
}

fn open_directory(path: &Path) -> Result<wasmi_wasi::Dir, WasmError> {
    wasmi_wasi::Dir::open_ambient_dir(path, wasmi_wasi::ambient_authority()).map_err(|error| {
        WasmError::CannotPrepare(format!("{} could not be opened: {error}", path.display()))
    })
}

/// Somewhere for a module's output to go.
///
/// In memory rather than to a file: a phone has nowhere convenient to put a
/// temporary file, and the output is small by construction — an application
/// that printed a gigabyte would exhaust its fuel long before its buffer.
#[derive(Debug, Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut held) = self.0.lock() {
            held.extend_from_slice(buffer);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sink {
    fn text(&self) -> String {
        self.0
            .lock()
            .map(|held| String::from_utf8_lossy(&held).into_owned())
            .unwrap_or_default()
    }
}

#[derive(Debug, Default)]
struct Captured {
    out: Sink,
    err: Sink,
}

impl Captured {
    fn new() -> Self {
        Self::default()
    }

    fn take(&self) -> (String, String) {
        (self.out.text(), self.err.text())
    }
}
