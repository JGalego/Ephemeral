//! Running a generated application inside WebAssembly.
//!
//! [ADR-0005](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0005-docker-first-runtime-abstraction.md)
//! made the runtime a trait with three implementations in mind, and
//! [ADR-0015](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0015-defer-the-native-runtime.md)
//! declined to build the native one on the grounds that a sandbox which cannot
//! enforce what its manifest declares is a sandbox in name only. That reasoning
//! has not changed. This is a different answer to the same question.
//!
//! ## Why this one is not the weak runtime ADR-0015 refused
//!
//! A native process starts with everything — the whole filesystem, the network,
//! every syscall — and confinement means taking it away, one platform-specific
//! mechanism at a time, and hoping the list is complete. That is the shape that
//! makes a "less isolated" label do more work than a label can.
//!
//! A WebAssembly module starts with **nothing**. It has no syscalls. It cannot
//! name a file, open a socket, read the clock or learn its own process id
//! unless the host hands it a function that does so. Confinement is not applied
//! to it; confinement is its resting state, and every capability is an explicit
//! addition. Forgetting to remove something yields *less* access rather than
//! more — which is the same property [`crate::ContainerSpec::minimal`] is built
//! around, enforced by the execution model rather than by remembering.
//!
//! That is also why it can run on a phone. There is no daemon, no process, no
//! namespace and no root: an interpreter in the host application's own memory
//! is the entire mechanism.
//!
//! ## What it costs
//!
//! Interpreted, not compiled — iOS forbids an application generating machine
//! code, so a just-in-time engine cannot ship there at all. Expect it to be
//! slower than a container by a wide margin. For the sort of thing Ephemeral
//! generates — read a file, count something, print an answer — that is a trade
//! worth making to have any runtime at all on a device that has none.
//!
//! And the application has to *be* WebAssembly. What that means for something a
//! model wrote in Python is a question for the layer above this one; what this
//! module owes it is a place to run where the permission model is real.

use std::path::PathBuf;
use std::time::Duration;

use crate::spec::{ContainerSpec, Egress};

mod engine;
mod program;
mod runtime;
mod session;

pub use engine::{Confined, Halt, WasmError, inspect, run};
pub use program::{NoProgram, PROGRAM_DIRECTORY, Program, languages};
pub use runtime::WasmRuntime;
pub use session::{Ran, Runnable, Shown, run as run_application};

/// What a module is allowed to do, derived from a [`ContainerSpec`].
///
/// The translation is the whole security argument, so it is a value that can be
/// inspected and asserted about rather than a sequence of calls buried in the
/// runtime. Every field here is something the specification granted; anything
/// the specification did not grant simply has no representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Directories the module may see, as (host path, the name it sees).
    ///
    /// WASI cannot name a path outside a preopened directory — not by policy,
    /// but because the only way to obtain a file descriptor is to derive it
    /// from one it already holds. A module given no preopens cannot open
    /// anything at all, which is the default here.
    pub visible: Vec<(PathBuf, String)>,

    /// Whether the module may write to any of them.
    pub writable: bool,

    /// The arguments, as the application sees them.
    pub arguments: Vec<String>,

    /// Environment variables, by name and value.
    ///
    /// Only the names a specification asked for, and only the values supplied
    /// at the moment of running — the same contract the container runtime has,
    /// for the same reason.
    pub environment: Vec<(String, String)>,

    /// How much work it may do before it is stopped.
    ///
    /// Not a wall clock. Fuel counts executed instructions, so a module cannot
    /// escape it by sleeping, blocking or being descheduled, and the bound is
    /// the same on a fast machine and a slow one — which is also why it is the
    /// only bound of this kind a phone can rely on.
    pub fuel: u64,

    /// How much memory it may allocate, in bytes.
    pub memory: usize,
}

/// How much work a module may do per second of CPU it was allowed.
///
/// Fuel is instructions and instructions are not seconds, so this is a
/// conversion with no exact answer. The number is measured rather than
/// imagined: a tight branch loop — the cheapest instruction there is, and so
/// the fastest a module can possibly go — runs at roughly 80 million fuel per
/// second under this interpreter. Real code, which touches memory, is slower
/// per instruction and therefore takes *longer* in wall clock for the same
/// fuel.
///
/// 50 million is that rate rounded down, which makes a second of declared CPU
/// mean somewhere between one and a few seconds of a person waiting. Being
/// generous here is not free: on a handset, the cost of a bound that is too
/// loose is a phone that appears to have stopped.
const FUEL_PER_CPU_SECOND: u64 = 50_000_000;

/// The longest a module runs where somebody is waiting for it.
///
/// A manifest may declare fifteen minutes, and on a desktop that is a job left
/// to get on with it. On a phone it is an application that has frozen. A caller
/// running something interactively passes this instead of what the manifest
/// says, and takes the smaller of the two.
pub const HANDHELD_CEILING: Duration = Duration::from_secs(30);

/// How much CPU time a specification allows, in whole seconds, at least one.
///
/// The product of the fraction of a core granted and how long it may run. At
/// least one second because a specification that works out to zero is one
/// nothing can run under, and refusing to start an application over a rounding
/// error is not a security property.
fn cpu_seconds(spec: &ContainerSpec, timeout: Duration) -> u64 {
    let millis = u64::from(spec.limits.cpu_millis).max(1);
    timeout
        .as_secs()
        .saturating_mul(millis)
        .saturating_div(1000)
        .max(1)
}

impl Capabilities {
    /// What a specification permits, and nothing else.
    ///
    /// # Panics
    ///
    /// Never. A mount whose host path cannot be named is dropped rather than
    /// guessed at, which fails closed.
    #[must_use]
    pub fn from_spec(spec: &ContainerSpec, timeout: Duration, secrets: &crate::Secrets) -> Self {
        // Egress has no representation here at all. wasmi's WASI has no sockets
        // to offer, so a module cannot reach the network whatever a
        // specification says — the one case where this runtime is *stricter*
        // than the container one, and the interface has to say so rather than
        // quietly appearing to honour a grant it cannot give effect to.
        let _ = Egress::Denied;

        let visible = spec
            .mounts
            .iter()
            .map(|mount| (mount.host_path.clone(), mount.container_path.clone()))
            .collect();

        Self {
            visible,
            writable: spec.mounts.iter().any(|mount| mount.writable),
            arguments: spec.entrypoint.clone(),
            environment: spec
                .environment_names
                .iter()
                .filter_map(|name| {
                    secrets
                        .get(name)
                        .map(|value| (name.clone(), value.to_owned()))
                })
                .collect(),
            // CPU-seconds, not seconds: a specification granting half a core
            // for a minute granted thirty seconds of work, and metering the
            // wall clock instead would silently double it.
            fuel: FUEL_PER_CPU_SECOND.saturating_mul(cpu_seconds(spec, timeout)),
            memory: usize::try_from(spec.limits.memory_mib)
                .unwrap_or(usize::MAX)
                .saturating_mul(1024 * 1024),
        }
    }

    /// Whether the module can reach anything of the user's at all.
    #[must_use]
    pub fn isolated(&self) -> bool {
        self.visible.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Compiles WebAssembly text into a module.
    ///
    /// Written as text rather than shipped as a binary fixture: a `.wasm` blob
    /// in the repository is a thing nobody can review, in the one place where
    /// what the bytes do is the entire question.
    fn module(text: &str) -> Vec<u8> {
        wat::parse_str(text).expect("the test module should assemble")
    }

    fn granted(fuel: u64) -> Capabilities {
        Capabilities {
            visible: Vec::new(),
            writable: false,
            arguments: Vec::new(),
            environment: Vec::new(),
            fuel,
            memory: 16 * 1024 * 1024,
        }
    }

    /// A module that does nothing runs, and is not mistaken for a failure.
    #[test]
    fn a_module_that_does_nothing_succeeds() {
        let wasm = module(r#"(module (func (export "_start")))"#);

        let outcome = run(&wasm, &granted(1_000_000)).expect("it runs");

        assert!(outcome.succeeded());
        assert!(outcome.output.is_empty());
    }

    /// **The capability model, in one test.**
    ///
    /// A module that imports a function the host did not provide cannot be
    /// instantiated. This is not a check Ephemeral performs — there is nothing
    /// for the import to bind to, so the module never starts. An application
    /// that wants a socket, or a host function somebody invented, fails at the
    /// door rather than halfway through doing something.
    #[test]
    fn a_module_asking_for_what_it_was_not_given_never_starts() {
        let wasm = module(
            r#"(module
                 (import "host" "exfiltrate" (func $out (param i32)))
                 (func (export "_start") (call $out (i32.const 1))))"#,
        );

        let refused = run(&wasm, &granted(1_000_000)).expect_err("it must not be allowed to start");

        assert!(
            matches!(refused, WasmError::Ungranted(_)),
            "expected a refusal about an ungranted capability, got {refused}"
        );
    }

    /// The same, for the one an application would actually reach for. wasmi's
    /// WASI has no sockets at all, so networking is not a permission this
    /// runtime can grant — it is a thing that does not exist here.
    #[test]
    fn there_is_no_socket_to_open() {
        let wasm = module(
            r#"(module
                 (import "wasi_snapshot_preview1" "sock_open"
                   (func $open (param i32 i32 i32) (result i32)))
                 (func (export "_start")
                   (drop (call $open (i32.const 0) (i32.const 0) (i32.const 0)))))"#,
        );

        let refused = run(&wasm, &granted(1_000_000)).expect_err("there is no socket to open");

        assert!(matches!(refused, WasmError::Ungranted(_)), "{refused}");
    }

    /// A module that never finishes is stopped, and the machine survives.
    ///
    /// Fuel counts instructions rather than seconds, so this bound cannot be
    /// escaped by sleeping or by being descheduled, and it is the same bound on
    /// a fast machine and a slow one.
    #[test]
    fn a_module_that_loops_forever_is_stopped() {
        let wasm = module(r#"(module (func (export "_start") (loop $forever (br $forever))))"#);

        let stopped = run(&wasm, &granted(100_000)).expect("being stopped is an outcome");

        assert!(!stopped.succeeded());
        assert_eq!(stopped.halted, Some(Halt::Processing));
        assert!(
            Halt::Processing
                .explain()
                .contains("Nothing was left running"),
            "and it says so in words somebody can read"
        );
    }

    /// Whatever a module said before it was stopped comes back.
    ///
    /// The reason being stopped is an outcome rather than an error: a program
    /// that reported its progress and then ran away is one whose output is the
    /// only thing that explains where it got to, and an error would have
    /// discarded it.
    #[test]
    fn what_it_said_before_it_was_stopped_is_not_thrown_away() {
        let wasm = module(
            r#"(module
                 (import "wasi_snapshot_preview1" "fd_write"
                   (func $write (param i32 i32 i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 8) "got this far\n")
                 (func (export "_start")
                   (i32.store (i32.const 0) (i32.const 8))
                   (i32.store (i32.const 4) (i32.const 13))
                   (drop (call $write
                     (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 20)))
                   (loop $forever (br $forever))))"#,
        );

        let stopped = run(&wasm, &granted(200_000)).expect("being stopped is an outcome");

        assert_eq!(stopped.halted, Some(Halt::Processing));
        assert_eq!(stopped.output, "got this far\n");
    }

    /// A module that allocates without end is stopped, and the phone survives.
    ///
    /// The bound this asserts was declared by [`Capabilities::memory`] for a
    /// while before anything applied it — a limit in a struct field is a
    /// promise, and this is the test that makes it a fact. One page is 64 KiB,
    /// so a ceiling of one page and a request for more is the smallest honest
    /// version of "it asked for the machine".
    #[test]
    fn a_module_that_allocates_without_end_is_stopped() {
        let wasm = module(
            r#"(module
                 (memory 1)
                 (func (export "_start")
                   (loop $more
                     (drop (memory.grow (i32.const 1)))
                     (br $more))))"#,
        );

        let stopped = run(
            &wasm,
            &Capabilities {
                memory: 64 * 1024,
                ..granted(100_000_000)
            },
        )
        .expect("being stopped is an outcome");

        assert!(!stopped.succeeded());
        assert_eq!(stopped.halted, Some(Halt::Memory));
    }

    /// And the same ceiling does not stop a module that stays under it. A bound
    /// that refuses everything is not a bound, it is a broken runtime, and the
    /// two look identical from the test above alone.
    #[test]
    fn a_module_that_stays_within_its_memory_is_left_alone() {
        let wasm = module(
            r#"(module
                 (memory 1)
                 (func (export "_start") (drop (memory.grow (i32.const 1)))))"#,
        );

        let outcome = run(
            &wasm,
            &Capabilities {
                memory: 4 * 64 * 1024,
                ..granted(1_000_000)
            },
        )
        .expect("two pages is inside a four page ceiling");

        assert!(outcome.succeeded());
        assert_eq!(outcome.halted, None, "nothing stopped it");
    }

    /// Output is captured rather than reaching the host's own streams. On a
    /// phone there is no terminal for it to reach, and on a desktop it belongs
    /// in the application's log rather than in Ephemeral's.
    #[test]
    fn what_it_prints_comes_back_rather_than_escaping() {
        // Writes "hi\n" to fd 1 through WASI, which is the only way out.
        let wasm = module(
            r#"(module
                 (import "wasi_snapshot_preview1" "fd_write"
                   (func $write (param i32 i32 i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 8) "hi\n")
                 (func (export "_start")
                   (i32.store (i32.const 0) (i32.const 8))
                   (i32.store (i32.const 4) (i32.const 3))
                   (drop (call $write
                     (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 20)))))"#,
        );

        let outcome = run(&wasm, &granted(1_000_000)).expect("it runs");

        assert_eq!(outcome.output, "hi\n");
    }

    /// A module that crashes is a crash, not a bound being hit, and not a
    /// success. An unrecognised stop has to land here rather than anywhere
    /// more comfortable, because the comfortable answers are all wrong.
    #[test]
    fn a_module_that_crashes_says_so() {
        let wasm = module(r#"(module (func (export "_start") (unreachable)))"#);

        let crashed = run(&wasm, &granted(1_000_000)).expect("a crash is an outcome");

        assert!(!crashed.succeeded());
        assert_eq!(crashed.halted, Some(Halt::Fault));
    }

    /// The exit codes a stopped module reports are the ones a container would
    /// have reported for the same event, so that nothing above the runtime
    /// needs to know which sandbox an application ran in.
    #[test]
    fn being_stopped_reports_the_exit_code_a_container_would_have() {
        assert_eq!(
            Halt::Processing.exit_code(),
            124,
            "what `timeout` exits with"
        );
        assert_eq!(Halt::Memory.exit_code(), 137, "a kill after running out");
        assert_ne!(Halt::Fault.exit_code(), 0, "and a crash is never a success");
    }

    /// Inspecting a module is the honest content of "build" for this runtime,
    /// and it is a real check rather than a formality.
    #[test]
    fn a_module_that_could_run_passes_inspection() {
        assert!(inspect(&module(r#"(module (func (export "_start")))"#)).is_ok());
    }

    #[test]
    fn bytes_that_are_not_a_module_do_not_pass_inspection() {
        let refused = inspect(b"this is a text file").expect_err("that is not WebAssembly");
        assert!(matches!(refused, WasmError::NotAModule(_)), "{refused}");
    }

    /// A library is not a program. Without this, an application would install
    /// cleanly and then fail at the moment somebody pressed Run.
    #[test]
    fn a_module_with_no_entry_point_does_not_pass_inspection() {
        let refused =
            inspect(&module(r#"(module (func (export "add")))"#)).expect_err("it has no `_start`");
        assert!(matches!(refused, WasmError::NotAModule(_)), "{refused}");
    }

    /// **The check worth having.** A module reaching for something nothing
    /// provides is caught before it is ever installed, rather than at the
    /// moment somebody tries to use it.
    #[test]
    fn a_module_asking_for_the_ungranted_fails_inspection() {
        let refused = inspect(&module(
            r#"(module
                 (import "host" "exfiltrate" (func $out (param i32)))
                 (func (export "_start") (call $out (i32.const 1))))"#,
        ))
        .expect_err("it asks for what nothing provides");

        assert!(matches!(refused, WasmError::Ungranted(_)), "{refused}");
    }

    /// Inspecting does not run the application. Checking a module by running it
    /// would mean running an unverified module to find out whether it should be
    /// run — so this asserts the program's own code never executed.
    #[test]
    fn inspecting_a_module_does_not_run_it() {
        let wasm = module(
            r#"(module
                 (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                 (memory (export "memory") 1)
                 (func (export "_start") (call $exit (i32.const 42))))"#,
        );

        // If `_start` had been called this would be an exit, not an `Ok`.
        assert!(inspect(&wasm).is_ok());
    }

    /// A specification that grants nothing produces capabilities that permit
    /// nothing. The default has to be the safe one, because forgetting is the
    /// common case.
    #[test]
    fn a_minimal_specification_can_reach_nothing() {
        let spec = ContainerSpec::minimal(
            ephemeral_core::AppId::parse("csv-comparator").expect("a valid id"),
            "unused",
            vec!["--help".to_owned()],
        );

        let capabilities =
            Capabilities::from_spec(&spec, Duration::from_secs(60), &crate::Secrets::new());

        assert!(capabilities.isolated(), "it can see nothing of the user's");
        assert!(capabilities.visible.is_empty());
        assert!(!capabilities.writable);
        assert!(capabilities.environment.is_empty());
        assert!(capabilities.fuel > 0, "and it still has room to run");
    }

    /// A specification granting half a core for a minute has granted thirty
    /// CPU-seconds, not sixty. Metering the wall clock instead would silently
    /// hand every application twice what its manifest says.
    #[test]
    fn the_allowance_is_cpu_time_rather_than_elapsed_time() {
        let mut half = ContainerSpec::minimal(
            ephemeral_core::AppId::parse("csv-comparator").expect("a valid id"),
            "unused",
            Vec::new(),
        );
        half.limits.cpu_millis = 500;

        let mut whole = half.clone();
        whole.limits.cpu_millis = 1000;

        let minute = Duration::from_secs(60);
        let secrets = crate::Secrets::new();

        assert_eq!(
            Capabilities::from_spec(&half, minute, &secrets).fuel * 2,
            Capabilities::from_spec(&whole, minute, &secrets).fuel,
            "twice the core is twice the work"
        );
    }

    /// A specification that rounds down to nothing still runs. Refusing to
    /// start an application over a rounding error is not a security property.
    #[test]
    fn an_allowance_too_small_to_measure_is_still_an_allowance() {
        let mut sliver = ContainerSpec::minimal(
            ephemeral_core::AppId::parse("csv-comparator").expect("a valid id"),
            "unused",
            Vec::new(),
        );
        sliver.limits.cpu_millis = 1;

        let capabilities =
            Capabilities::from_spec(&sliver, Duration::from_secs(1), &crate::Secrets::new());

        assert!(capabilities.fuel > 0);
    }

    /// A granted folder becomes a directory the module can name, and nothing
    /// else does. This is the translation the whole security argument rests on,
    /// so it is asserted rather than assumed.
    #[test]
    fn a_granted_folder_is_the_only_one_it_can_name() {
        let home = tempfile::tempdir().expect("a temporary directory");

        let mut spec = ContainerSpec::minimal(
            ephemeral_core::AppId::parse("csv-comparator").expect("a valid id"),
            "unused",
            Vec::new(),
        );
        spec.mounts
            .push(crate::spec::Mount::read_only(home.path(), "/mnt/data"));

        let capabilities =
            Capabilities::from_spec(&spec, Duration::from_secs(60), &crate::Secrets::new());

        assert_eq!(
            capabilities.visible,
            vec![(home.path().to_path_buf(), "/mnt/data".to_owned())]
        );
        assert!(!capabilities.isolated());
        assert!(
            !capabilities.writable,
            "a read-only grant stays read-only across the translation"
        );
    }

    /// Only the settings a specification named, and only when a value was
    /// supplied. A name with no value is not passed as empty — it is absent,
    /// which is what the application can then complain about.
    #[test]
    fn only_the_named_settings_are_passed() {
        let mut spec = ContainerSpec::minimal(
            ephemeral_core::AppId::parse("csv-comparator").expect("a valid id"),
            "unused",
            Vec::new(),
        );
        spec.environment_names = vec!["WANTED".to_owned(), "NEVER_SUPPLIED".to_owned()];

        let mut secrets = crate::Secrets::new();
        secrets.insert("WANTED", "a-value");
        secrets.insert("NOT_ASKED_FOR", "must not appear");

        let capabilities = Capabilities::from_spec(&spec, Duration::from_secs(60), &secrets);

        assert_eq!(
            capabilities.environment,
            vec![("WANTED".to_owned(), "a-value".to_owned())]
        );
    }
}
