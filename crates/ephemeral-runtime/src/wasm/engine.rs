//! The interpreter, and the four bounds it enforces.
//!
//! Everything here exists to make one sentence true: *a generated application
//! gets what it was granted and nothing else.* The four ways that could fail
//! are the four things this module bounds — reaching outside its directories,
//! reaching the network, running forever, and allocating without end. Each has
//! a test that tries it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmi_wasi::WasiCtx;

use super::Capabilities;

/// What a module did, and what it said while doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confined {
    /// How it exited.
    ///
    /// Carried rather than reduced to a boolean, because a program's exit code
    /// is a thing it chose to say. When [`Confined::halted`] is set this is the
    /// code the equivalent container would have reported, so a caller can treat
    /// both runtimes' answers the same way.
    pub exit_code: i32,

    /// What it wrote to standard output.
    pub output: String,

    /// What it wrote to standard error.
    pub diagnostics: String,

    /// Which bound stopped it, if one did.
    ///
    /// `None` means it reached its own end — well or badly, but on its own
    /// terms. Anything else means Ephemeral ended it.
    pub halted: Option<Halt>,
}

impl Confined {
    /// Whether it did what it was asked, on its own terms.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && self.halted.is_none()
    }
}

/// Why Ephemeral ended a module rather than letting it finish.
///
/// Being stopped is an outcome, not a failure to run, so it travels in
/// [`Confined`] rather than in [`WasmError`]. That is not a nicety: a module
/// that printed something useful and *then* looped forever has its output kept
/// this way, and an error would have thrown away the only thing that explains
/// what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Halt {
    /// It used its whole processing allowance.
    Processing,

    /// It asked for more memory than it was allowed.
    Memory,

    /// It did something WebAssembly does not permit — divided by zero, read
    /// past the end of its own memory, reached an `unreachable`.
    Fault,
}

impl Halt {
    /// What to tell a person, in words that do not blame them.
    #[must_use]
    pub fn explain(self) -> &'static str {
        match self {
            Self::Processing => {
                "It used more processing than it was allowed and was stopped. \
                 Nothing was left running."
            }
            Self::Memory => {
                "It asked for more memory than it was allowed and was stopped. \
                 Nothing was left running."
            }
            Self::Fault => "It crashed. Nothing was left running.",
        }
    }

    /// The exit code to report, matching what a container would have reported.
    ///
    /// 124 is what `timeout` exits with and 137 is a kill after running out of
    /// memory, which is what the Docker runtime surfaces for the same two
    /// events. Everything above the runtime layer therefore needs no special
    /// case for which sandbox an application happened to run in.
    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Processing => 124,
            Self::Memory => 137,
            Self::Fault => 134,
        }
    }
}

/// Why a module could not be run at all.
///
/// Every variant here means **nothing executed**. A module that ran and failed,
/// or ran and was stopped, is a [`Confined`] — for the same reason a failing
/// test is an answer rather than a fault.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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

    /// The host could not set something up.
    #[error("the sandbox could not be prepared: {0}")]
    CannotPrepare(String),
}

/// Runs one module under exactly `capabilities`, and no more.
///
/// # Errors
///
/// [`WasmError::Ungranted`] when the module imports something it was not given,
/// and [`WasmError::NotAModule`] when the bytes do not load. Both mean nothing
/// ran. A module that runs and exits non-zero, or that is stopped for exceeding
/// a bound, is **not** an error — that is a [`Confined`] with `succeeded` false
/// and, in the second case, a [`Halt`].
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
    let sandboxed = Sandboxed {
        wasi: build_context(capabilities, &captured)?,
        // Applied per linear memory. A module that grows past it traps rather
        // than being told the growth failed, because a generated application
        // handling `memory.grow` returning -1 gracefully is not something to
        // rely on: the honest outcome of exceeding a bound is being stopped.
        limits: StoreLimitsBuilder::new()
            .memory_size(capabilities.memory)
            .trap_on_grow_failure(true)
            .build(),
    };

    let mut store = Store::new(&engine, sandboxed);
    store.limiter(|sandboxed| &mut sandboxed.limits);
    store
        .set_fuel(capabilities.fuel)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let mut linker = <Linker<Sandboxed>>::new(&engine);
    // The *only* imports this module will be able to resolve. Nothing else is
    // added, so anything the application asks for beyond WASI — a socket, a
    // host function somebody invented — has nothing to bind to and the
    // instantiation below fails.
    wasmi_wasi::add_to_linker(&mut linker, |sandboxed: &mut Sandboxed| &mut sandboxed.wasi)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let instance = linker
        .instantiate(&mut store, &module)
        .and_then(|pre| pre.start(&mut store))
        .map_err(|error| refusal(&error))?;

    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|_| {
            WasmError::NotAModule(
                "it has no `_start`, so it is a library rather than a program".to_owned(),
            )
        })?;

    let outcome = start.call(&mut store, ());
    let (output, diagnostics) = captured.take();

    let Err(error) = outcome else {
        return Ok(Confined {
            exit_code: 0,
            output,
            diagnostics,
            halted: None,
        });
    };

    // An exit code is how a WASI program says it finished, well or badly. That
    // is a result rather than a malfunction, and treating it as an error would
    // report every failing test as a broken sandbox.
    if let Some(status) = error.i32_exit_status() {
        return Ok(Confined {
            exit_code: status,
            output,
            diagnostics,
            halted: None,
        });
    }

    // Compilation is lazy, so a module that cannot run at all surfaces here
    // rather than at instantiation. It is the same refusal wherever it is
    // discovered, and calling it a crash would send somebody debugging a
    // program that never started.
    if let Some(refused) = unrunnable(&error) {
        return Err(refused);
    }

    let stopped = halt(&error);
    Ok(Confined {
        exit_code: stopped.exit_code(),
        output,
        diagnostics,
        halted: Some(stopped),
    })
}

/// Checks that a module is one this build could actually run, without running
/// it.
///
/// The honest content of "build" for this runtime. A container application is
/// made ready by building an image, which fails loudly when the source is
/// wrong; a WebAssembly application arrives already built, and the equivalent
/// question is whether the bytes are a program at all. That is not a formality:
/// this catches a file that is not WebAssembly, one compiled for something
/// other than WASI, one with no entry point, and — the important one — one that
/// imports a capability nothing here provides.
///
/// It instantiates but does not call `_start`, so the application's own code
/// never executes. Checking a module by running it would mean running an
/// unverified module to find out whether it should be run.
///
/// # Errors
///
/// [`WasmError::NotAModule`] when the bytes do not load or there is no entry
/// point, and [`WasmError::Ungranted`] when it reaches for something it was not
/// given.
pub fn inspect(wasm: &[u8]) -> Result<(), WasmError> {
    let mut config = Config::default();
    config.compilation_mode(wasmi::CompilationMode::Lazy);
    config.consume_fuel(true);

    let engine = Engine::new(&config);
    let module =
        Module::new(&engine, wasm).map_err(|error| WasmError::NotAModule(error.to_string()))?;

    let captured = Captured::new();
    let nothing = Capabilities {
        visible: Vec::new(),
        writable: false,
        arguments: Vec::new(),
        environment: Vec::new(),
        // Enough for a start section and no more. A module that wants to do
        // real work before `main` is not being inspected, it is being run.
        fuel: 1_000_000,
        memory: 16 * 1024 * 1024,
    };

    let sandboxed = Sandboxed {
        wasi: build_context(&nothing, &captured)?,
        limits: StoreLimitsBuilder::new()
            .memory_size(nothing.memory)
            .trap_on_grow_failure(true)
            .build(),
    };

    let mut store = Store::new(&engine, sandboxed);
    store.limiter(|sandboxed| &mut sandboxed.limits);
    store
        .set_fuel(nothing.fuel)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let mut linker = <Linker<Sandboxed>>::new(&engine);
    wasmi_wasi::add_to_linker(&mut linker, |sandboxed: &mut Sandboxed| &mut sandboxed.wasi)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let instance = linker
        .instantiate(&mut store, &module)
        .and_then(|pre| pre.start(&mut store))
        .map_err(|error| refusal(&error))?;

    instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map(|_| ())
        .map_err(|_| {
            WasmError::NotAModule(
                "it has no `_start`, so it is a library rather than a program".to_owned(),
            )
        })
}

/// Everything one run's store holds.
///
/// The WASI context and the allocation bounds travel together because wasmi
/// asks for both through the store's data type: a limiter is a function from
/// that type, so a store whose data is a bare [`WasiCtx`] is a store that can
/// have no memory limit at all.
struct Sandboxed {
    wasi: WasiCtx,
    limits: StoreLimits,
}

/// The refusal a failure represents, if it is one.
///
/// Kept apart from [`halt`] because the two need different words: one is
/// Ephemeral refusing, and the other is an application that did something
/// wrong. Presenting a refusal as a crash would teach somebody to distrust the
/// sandbox.
fn unrunnable(error: &wasmi::Error) -> Option<WasmError> {
    let said = error.to_string();

    if said.contains("unknown import") || said.contains("cannot find") {
        return Some(WasmError::Ungranted(said));
    }

    // WASI reaches into the module's linear memory to read and write anything
    // larger than a number, so a module that exports none is not a WASI program
    // however well it compiles. Reporting it as a crash would send somebody
    // debugging a program that never ran an instruction.
    if said.contains("missing required WASI memory export") {
        return Some(WasmError::NotAModule(
            "it exports no memory, so it is not a program this build can run. \
             It was probably compiled for something other than WASI."
                .to_owned(),
        ));
    }

    None
}

/// A failure before the program ever started, classified.
fn refusal(error: &wasmi::Error) -> WasmError {
    unrunnable(error).unwrap_or_else(|| WasmError::CannotPrepare(error.to_string()))
}

/// Which bound a running module hit.
///
/// Matched on the interpreter's own words, which is unlovely and is what is
/// available: wasmi reports a trap as a message rather than as a code. The
/// default is [`Halt::Fault`] — an unrecognised stop is a crash rather than a
/// success, so a message this function has never seen fails closed.
fn halt(error: &wasmi::Error) -> Halt {
    let said = error.to_string();

    if said.contains("fuel") {
        return Halt::Processing;
    }
    // What wasmi says when the store's limiter refuses an allocation. Reading
    // past the end of one's own memory is *not* this: that is a program bug,
    // and calling it "you were given too little memory" would send somebody
    // raising a limit that was never the problem.
    if said.contains("growth operation limited") {
        return Halt::Memory;
    }

    Halt::Fault
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
