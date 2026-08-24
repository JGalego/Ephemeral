//! The interpreter, and the four bounds it enforces.
//!
//! Everything here exists to make one sentence true: *a generated application
//! gets what it was granted and nothing else.* The four ways that could fail
//! are the four things this module bounds — reaching outside its directories,
//! reaching the network, running forever, and allocating without end. Each has
//! a test that tries it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use wasmi::{Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmi_wasi::WasiCtx;

use ephemeral_core::permission::HostScope;

use super::{Answered, Capabilities, MOST_ONE_BODY, Method, Outbound, Reach};

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
pub fn run(
    wasm: &[u8],
    capabilities: &Capabilities,
    reach: Option<&dyn Reach>,
) -> Result<Confined, WasmError> {
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
        capabilities,
        reach,
        remaining: capabilities.requests,
        answered: Vec::new(),
    };

    let mut store = Store::new(&engine, sandboxed);
    store.limiter(|sandboxed| &mut sandboxed.limits);
    store
        .set_fuel(capabilities.fuel)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let mut linker = <Linker<Sandboxed<'_>>>::new(&engine);
    // The *only* imports this module will be able to resolve. Nothing else is
    // added, so anything the application asks for beyond WASI and — where it
    // was granted — the two functions below has nothing to bind to, and the
    // instantiation further down fails.
    wasmi_wasi::add_to_linker(&mut linker, |sandboxed: &mut Sandboxed<'_>| {
        &mut sandboxed.wasi
    })
    .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    // Always linked, and what varies is what they will do.
    //
    // These used to be linked only when egress had been granted, so that a
    // module importing them without a grant could not start at all. That is a
    // pleasing property for a module that *is* an application — its imports are
    // a truthful declaration of what it needs. It is a lie for an interpreter,
    // which is one module running somebody else's script: the JavaScript
    // interpreter imports these because *some* script might use them, and
    // refusing to start it would mean every scripted application needed a
    // network grant to print a line.
    //
    // Nothing is weakened by this. The grant was never enforced by the linker;
    // it is enforced per request in `answer`, against the ledger, before
    // anything is asked to carry it. What an ungranted application gets now is
    // a refusal it can show somebody, rather than a door that will not open.
    link_network(&mut linker)?;

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
        writable: Vec::new(),
        // Checking is not running, and an inspection that reached the network
        // would be an inspection with a side effect. A module that imports the
        // network functions is checked below against the grant it will actually
        // be given, not against this.
        reachable: crate::spec::Egress::Denied,
        requests: 0,
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
        capabilities: &nothing,
        reach: None,
        remaining: 0,
        answered: Vec::new(),
    };

    let mut store = Store::new(&engine, sandboxed);
    store.limiter(|sandboxed| &mut sandboxed.limits);
    store
        .set_fuel(nothing.fuel)
        .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    let mut linker = <Linker<Sandboxed<'_>>>::new(&engine);
    wasmi_wasi::add_to_linker(&mut linker, |sandboxed: &mut Sandboxed<'_>| {
        &mut sandboxed.wasi
    })
    .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;

    // Present so that an application which asks for the network is not called
    // broken by a check that never intended to give it one. Whether it may
    // actually reach anything is decided when it runs, from the grant.
    link_network(&mut linker)?;

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
struct Sandboxed<'a> {
    wasi: WasiCtx,
    limits: StoreLimits,

    /// What the person granted. Consulted on every request, never cached into
    /// a decision made once at start-up.
    capabilities: &'a Capabilities,

    /// Whoever will actually make a request. `None` when nothing was granted,
    /// in which case no host function is linked and this is never read.
    reach: Option<&'a dyn Reach>,

    /// How many requests are left in this run.
    remaining: u32,

    /// The last answer, waiting to be copied into the module's memory.
    ///
    /// Held on this side rather than written straight into linear memory,
    /// because the module has to be told how much room to make before it can
    /// offer any. One slot, not a table: an application can have exactly one
    /// request in flight, which is all a synchronous host call can mean.
    answered: Vec<u8>,
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
        // Named in the words a person granted it in. The interpreter's own
        // sentence — "cannot find definition for import ephemeral::send with
        // type Func(FuncType { … })" — is true and tells somebody holding a
        // phone nothing they can act on, when what happened is that an
        // application asked to use the network and nobody said it could.
        if said.contains(NETWORK_MODULE) {
            return Some(WasmError::Ungranted(
                "it needs to reach a service over the network, and has not been allowed \
                 to reach anything. Allowing it says which service."
                    .to_owned(),
            ));
        }
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

/// The name of the import module an application reaches the network through.
pub(super) const NETWORK_MODULE: &str = "ephemeral";

/// Asks for one request. Returns how many bytes the answer is, or a negative
/// number when the request itself could not be read.
pub(super) const SEND: &str = "send";

/// Copies the answer out. Returns how many bytes were copied.
pub(super) const RECV: &str = "recv";

/// The request could not be read: it is not JSON, names a method that is not
/// one of the two, or does not fit.
const UNREADABLE: i32 = -1;

/// The pointer and length an application gave do not lie inside its own memory.
const OUT_OF_BOUNDS: i32 = -2;

/// Gives a module the two functions it reaches the network through.
///
/// ## Why two, and why bytes
///
/// A host call cannot return a variable-length value, so the answer is held on
/// this side and the module is told how big it is. It makes room, then asks for
/// it. The alternative — the module guessing a buffer size and the host
/// truncating — makes silent partial answers, which for a message from another
/// person is the worst possible failure.
///
/// ## What an application can and cannot say
///
/// A request is `{"method":"GET"|"POST","url":"…","body":"…"}` and nothing
/// else. **No headers.** An application that could set a header on a request
/// the host performs is an application that can attach a credential it was
/// never shown, to a destination of its choosing; nothing a generated
/// application legitimately does needs one.
///
/// Every answer is a readable document, refusals included:
/// `{"status":200,"body":"…"}` or `{"status":0,"error":"…"}`. An application
/// told "you are not allowed to reach that" can say so to the person looking at
/// it, which is a great deal more useful than a trap.
fn link_network(linker: &mut Linker<Sandboxed<'_>>) -> Result<(), WasmError> {
    fn prepare(error: impl std::fmt::Display) -> WasmError {
        WasmError::CannotPrepare(error.to_string())
    }

    linker
        .func_wrap(
            NETWORK_MODULE,
            SEND,
            |mut caller: Caller<'_, Sandboxed<'_>>, at: i32, len: i32| -> i32 {
                let Some(asked) = borrow(&caller, at, len) else {
                    return OUT_OF_BOUNDS;
                };

                let answered = answer(caller.data_mut(), &asked);
                let Ok(written) = serde_json::to_vec(&answered) else {
                    return UNREADABLE;
                };

                let length = i32::try_from(written.len()).unwrap_or(i32::MAX);
                caller.data_mut().answered = written;
                length
            },
        )
        .map_err(prepare)?;

    linker
        .func_wrap(
            NETWORK_MODULE,
            RECV,
            |mut caller: Caller<'_, Sandboxed<'_>>, at: i32, room: i32| -> i32 {
                // Taken rather than copied: one answer is delivered once, so a
                // module cannot be handed a stale reply by asking twice.
                let answered = std::mem::take(&mut caller.data_mut().answered);
                let room = usize::try_from(room).unwrap_or(0);
                if answered.len() > room {
                    return OUT_OF_BOUNDS;
                }

                match place(&mut caller, at, &answered) {
                    Some(()) => i32::try_from(answered.len()).unwrap_or(i32::MAX),
                    None => OUT_OF_BOUNDS,
                }
            },
        )
        .map_err(prepare)?;

    Ok(())
}

/// What one request gets back, refusals included.
///
/// Split out from the host function so the whole decision — is it readable, is
/// it allowed, is there anything left in the budget — is a function from data
/// to data, and can be asserted about without an interpreter.
fn answer(sandboxed: &mut Sandboxed<'_>, asked: &[u8]) -> serde_json::Value {
    let refused = |reason: String| serde_json::json!({ "status": 0, "error": reason });

    if asked.len() > MOST_ONE_BODY {
        return refused(format!(
            "that request is larger than the {MOST_ONE_BODY} bytes an application may send"
        ));
    }

    let Ok(asked) = serde_json::from_slice::<serde_json::Value>(asked) else {
        return refused("that is not a request this runtime understands".to_owned());
    };

    let Some(method) = asked
        .get("method")
        .and_then(serde_json::Value::as_str)
        .and_then(Method::of)
    else {
        return refused("a request is a GET or a POST, and says which".to_owned());
    };

    let url = asked
        .get("url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let Some(destination) = destination(&url) else {
        return refused(format!(
            "{url} is not somewhere this runtime can be asked to reach"
        ));
    };

    // The grant, checked here rather than by whoever performs the request.
    if !sandboxed.capabilities.may_reach(&destination) {
        return refused(format!(
            "this application was not allowed to reach {destination}. It has {}.",
            sandboxed.capabilities.reachable.describe()
        ));
    }

    if sandboxed.remaining == 0 {
        return refused(
            "it has made every request it was allowed in one run. \
             Nothing was sent."
                .to_owned(),
        );
    }
    sandboxed.remaining -= 1;

    let Some(reach) = sandboxed.reach else {
        return refused("nothing here can carry a request".to_owned());
    };

    let body = match method {
        Method::Get => String::new(),
        Method::Post => asked
            .get("body")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };

    match reach.fetch(&Outbound { method, url, body }) {
        Ok(Answered { status, body }) if body.len() > MOST_ONE_BODY => serde_json::json!({
            "status": status,
            "error": format!(
                "the reply was larger than the {MOST_ONE_BODY} bytes an application may be handed"
            ),
        }),
        Ok(Answered { status, body }) => serde_json::json!({ "status": status, "body": body }),
        Err(reason) => refused(reason),
    }
}

/// The host and port a URL names, or nothing when it names none.
///
/// Deliberately strict, and deliberately not a URL parser: this crate has no
/// business normalising somebody's URL, and every ambiguity here is a way for
/// two pieces of code to disagree about which host a request reaches. Only
/// `https://` and `http://` are recognised; anything with a credential in it —
/// `https://user@host/` — is refused rather than untangled.
fn destination(url: &str) -> Option<HostScope> {
    let rest = url
        .strip_prefix("https://")
        .map(|rest| (rest, 443))
        .or_else(|| url.strip_prefix("http://").map(|rest| (rest, 80)));
    let (rest, default_port) = rest?;

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;
    if authority.contains('@') {
        return None;
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, default_port),
    };

    HostScope::parse(format!("{host}:{port}")).ok()
}

/// A module's own memory, if it exports one.
fn memory(caller: &Caller<'_, Sandboxed<'_>>) -> Option<wasmi::Memory> {
    match caller.get_export("memory") {
        Some(wasmi::Extern::Memory(memory)) => Some(memory),
        _ => None,
    }
}

/// Reads `len` bytes at `at` out of the module's memory.
fn borrow(caller: &Caller<'_, Sandboxed<'_>>, at: i32, len: i32) -> Option<Vec<u8>> {
    let memory = memory(caller)?;
    let at = usize::try_from(at).ok()?;
    let len = usize::try_from(len).ok()?;

    memory
        .data(caller)
        .get(at..at.checked_add(len)?)
        .map(<[u8]>::to_vec)
}

/// Writes `bytes` into the module's memory at `at`.
fn place(caller: &mut Caller<'_, Sandboxed<'_>>, at: i32, bytes: &[u8]) -> Option<()> {
    let memory = memory(caller)?;
    let at = usize::try_from(at).ok()?;

    memory.write(caller, at, bytes).ok()
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

    // Built before the preopens are pushed, because a read-only mount cannot go
    // through the builder at all: `preopened_dir` takes a `cap_std` directory
    // and wraps it in the read-write implementation itself. The context's own
    // `push_preopened_dir` takes any `WasiDir`, which is what lets a mount be
    // what the specification says it is.
    let context = builder.build();

    for (host, seen_as) in &capabilities.visible {
        let opened = wasmi_wasi::dir::Dir::from_cap_std(open_directory(host)?);

        // Read-only means read-only. Every mount used to be preopened with the
        // ambient authority it was opened with, whatever the specification
        // said, so an application whose write grant had been revoked could
        // still write — while the run's banner correctly said "Can read" and
        // the sentence shown when granting says "It cannot change those files".
        let mounted: Box<dyn wasmi_wasi::WasiDir> = if capabilities.may_write(seen_as) {
            Box::new(opened)
        } else {
            Box::new(super::readonly::ReadOnly(Box::new(opened)))
        };

        context
            .push_preopened_dir(mounted, seen_as)
            .map_err(|error| WasmError::CannotPrepare(error.to_string()))?;
    }

    Ok(context)
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
