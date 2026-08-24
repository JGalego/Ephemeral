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

use ephemeral_core::permission::HostScope;

use crate::spec::{ContainerSpec, Egress};

mod engine;
mod program;
mod reach;
mod readonly;
mod runtime;
mod session;

pub use engine::{Confined, Halt, WasmError, inspect, run};
pub use program::{NoProgram, PROGRAM_DIRECTORY, Program, languages};
pub use reach::{Answered, MOST_ONE_BODY, MOST_REQUESTS, Method, Outbound, Reach};
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

    /// Which of them it may write to, by the name it sees.
    ///
    /// A separate list rather than a flag on each entry, so the question the
    /// engine asks — *may this preopen be written to?* — has one answer and one
    /// place to look. The version of this that carried a single `writable`
    /// summarising every mount is why a revoked write grant still permitted
    /// writes: the summary was reported to a person and never consulted when
    /// the directory was opened.
    pub writable: Vec<String>,

    /// Where it may send a request, if anywhere.
    ///
    /// [`Egress::Denied`] — the default — means no host function is linked at
    /// all, so a module that imports one cannot start. Anything else means the
    /// engine will check each request against this before handing it to a
    /// [`Reach`].
    pub reachable: Egress,

    /// How many requests one run may make.
    pub requests: u32,

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
        let visible = spec
            .mounts
            .iter()
            .map(|mount| (mount.host_path.clone(), mount.container_path.clone()))
            .collect();

        Self {
            visible,
            // Only the mounts the specification actually made writable. Every
            // other one is opened read-only, which is what the sentence a
            // person was shown when they granted it — "It cannot change those
            // files" — has always claimed.
            writable: spec
                .mounts
                .iter()
                .filter(|mount| mount.writable)
                .map(|mount| mount.container_path.clone())
                .collect(),
            // Carried rather than dropped. There are still no sockets: what an
            // egress grant now buys is that the engine will link a host
            // function, and ask whoever is running the application to make the
            // request. Denied stays the default and links nothing.
            reachable: spec.egress.clone(),
            requests: MOST_REQUESTS,
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
        self.visible.is_empty() && !self.reachable.is_permitted()
    }

    /// Whether the module may write to the preopen it sees as `seen_as`.
    #[must_use]
    pub fn may_write(&self, seen_as: &str) -> bool {
        self.writable.iter().any(|writable| writable == seen_as)
    }

    /// Whether a request to `host` is one the grant covers.
    ///
    /// Asked here rather than by whoever performs the request. A host that
    /// decided this for itself would be a second copy of the permission model,
    /// in another language, on another release cycle — and the copy that drifts
    /// is the one nobody is looking at.
    #[must_use]
    pub fn may_reach(&self, host: &HostScope) -> bool {
        match &self.reachable {
            Egress::Denied => false,
            Egress::Anywhere => true,
            Egress::AllowList(allowed) => allowed.iter().any(|scope| scope.contains(host)),
        }
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
            writable: Vec::new(),
            reachable: Egress::Denied,
            requests: MOST_REQUESTS,
            arguments: Vec::new(),
            environment: Vec::new(),
            fuel,
            memory: 16 * 1024 * 1024,
        }
    }

    /// Escapes text so it can sit in a WebAssembly text data segment.
    fn quoted(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// A module that tries to create a file in the first preopened directory,
    /// and exits with the errno it got — zero when it was allowed.
    fn tries_to_write() -> Vec<u8> {
        module(
            r#"(module
                 (import "wasi_snapshot_preview1" "path_open"
                   (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
                 (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 100) "note.txt")
                 (func (export "_start")
                   (call $exit
                     (call $open
                       (i32.const 3)      ;; the first preopen
                       (i32.const 0)
                       (i32.const 100)    ;; "note.txt"
                       (i32.const 8)
                       (i32.const 1)      ;; O_CREAT
                       (i64.const 64)     ;; FD_WRITE
                       (i64.const 64)
                       (i32.const 0)
                       (i32.const 200)))))"#,
        )
    }

    /// What WASI calls "operation not permitted".
    const EPERM: i32 = 63;

    /// **A read-only mount is read-only.**
    ///
    /// It was not. Every mount was preopened with the ambient authority its
    /// directory was opened with, whatever the specification said, so an
    /// application whose write permission had been *revoked* could still
    /// write — while the run's banner correctly said "Can read" and the
    /// sentence shown when granting read says "It cannot change those files".
    ///
    /// Found by doing exactly this to a real application, not by reading the
    /// code: nothing here distinguished the two mounts, so nothing failed.
    #[test]
    fn a_read_only_mount_refuses_to_be_written_to() {
        let folder = tempfile::tempdir().expect("a temporary directory");

        let refused = run(
            &tries_to_write(),
            &Capabilities {
                visible: vec![(folder.path().to_path_buf(), "/mnt/data".to_owned())],
                writable: Vec::new(),
                ..granted(10_000_000)
            },
            None,
        )
        .expect("being refused is an outcome, not a failure to start");

        assert_eq!(
            refused.exit_code, EPERM,
            "it should not have been permitted to create anything"
        );
        assert!(
            !folder.path().join("note.txt").exists(),
            "and nothing should have appeared on disk"
        );
    }

    /// And a writable one still works. A sandbox that refuses everything is not
    /// a sandbox, it is a broken runtime, and the test above alone cannot tell
    /// the two apart.
    #[test]
    fn a_writable_mount_can_still_be_written_to() {
        let folder = tempfile::tempdir().expect("a temporary directory");

        let allowed = run(
            &tries_to_write(),
            &Capabilities {
                visible: vec![(folder.path().to_path_buf(), "/mnt/data".to_owned())],
                writable: vec!["/mnt/data".to_owned()],
                ..granted(10_000_000)
            },
            None,
        )
        .expect("it runs");

        assert_eq!(allowed.exit_code, 0, "it was allowed to create a file");
        assert!(folder.path().join("note.txt").exists());
    }

    /// A module that asks for one request and prints whatever came back.
    fn asks_for(request: &str) -> Vec<u8> {
        module(&format!(
            r#"(module
                 (import "wasi_snapshot_preview1" "fd_write"
                   (func $write (param i32 i32 i32 i32) (result i32)))
                 (import "ephemeral" "send" (func $send (param i32 i32) (result i32)))
                 (import "ephemeral" "recv" (func $recv (param i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 1024) "{escaped}")
                 (func (export "_start")
                   (local $n i32)
                   (drop (call $send (i32.const 1024) (i32.const {length})))
                   (local.set $n (call $recv (i32.const 2048) (i32.const 8192)))
                   (i32.store (i32.const 0) (i32.const 2048))
                   (i32.store (i32.const 4) (local.get $n))
                   (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1)
                                      (i32.const 8)))))"#,
            escaped = quoted(request),
            length = request.len(),
        ))
    }

    /// A `Reach` that answers everything the same way, and remembers what it
    /// was asked for.
    #[derive(Default)]
    struct Answers {
        asked: std::sync::Mutex<Vec<Outbound>>,
    }

    impl Reach for Answers {
        fn fetch(&self, request: &Outbound) -> Result<Answered, String> {
            self.asked
                .lock()
                .expect("nothing else holds this")
                .push(request.clone());
            Ok(Answered {
                status: 200,
                body: "pong".to_owned(),
            })
        }
    }

    fn reaching(host: &str) -> Capabilities {
        Capabilities {
            reachable: Egress::AllowList(vec![
                ephemeral_core::permission::HostScope::parse(host).expect("a host"),
            ]),
            ..granted(50_000_000)
        }
    }

    /// **An application with no network grant cannot even start one.**
    ///
    /// The capability model, and the reason it is worth having: the host
    /// functions are not linked at all, so a module that imports them has
    /// nothing to bind to and never executes an instruction. There is no
    /// version of this where the application runs and the request quietly
    /// fails.
    #[test]
    fn a_module_that_asks_for_the_network_without_a_grant_never_starts() {
        let wasm = asks_for(r#"{"method":"GET","url":"https://api.example.com/ping"}"#);
        let carrier = Answers::default();

        let refused =
            run(&wasm, &granted(50_000_000), Some(&carrier)).expect_err("it was granted nothing");

        assert!(matches!(refused, WasmError::Ungranted(_)), "{refused}");
        assert!(
            carrier
                .asked
                .lock()
                .expect("nothing else holds this")
                .is_empty(),
            "and nothing was sent"
        );
    }

    /// **A granted application reaches the host it was granted.**
    #[test]
    fn a_granted_request_is_carried_and_the_answer_comes_back() {
        let wasm = asks_for(r#"{"method":"GET","url":"https://api.example.com/ping"}"#);
        let carrier = Answers::default();

        let outcome = run(&wasm, &reaching("api.example.com"), Some(&carrier)).expect("it runs");

        assert!(outcome.succeeded(), "{outcome:?}");
        assert!(
            outcome.output.contains("pong") && outcome.output.contains("200"),
            "the answer reaches the application: {}",
            outcome.output
        );

        let asked = carrier.asked.lock().expect("nothing else holds this");
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].method, Method::Get);
        assert_eq!(asked[0].url, "https://api.example.com/ping");
        assert!(asked[0].body.is_empty(), "a GET carries no body");
    }

    /// **And nothing else.**
    ///
    /// The grant names one host. Every other destination is refused *before*
    /// anything is asked to carry it — so a host implementation that would
    /// happily fetch anything is never given the chance, which is the whole
    /// reason the check lives here and not there.
    #[test]
    fn a_host_that_was_not_granted_is_refused_before_anything_is_sent() {
        let carrier = Answers::default();

        for elsewhere in [
            "https://api.example.org/ping",
            "https://evil.example.com.attacker.net/ping",
            "http://api.example.com/ping", // the same name, the wrong port
            "https://api.example.com:8443/ping",
            "file:///etc/passwd",
            "https://user@api.example.com/ping",
        ] {
            let wasm = asks_for(&format!(r#"{{"method":"GET","url":"{elsewhere}"}}"#));
            let outcome = run(&wasm, &reaching("api.example.com:443"), Some(&carrier))
                .expect("a refusal is an answer, not a crash");

            assert!(
                outcome.output.contains("error"),
                "{elsewhere} should have been refused: {}",
                outcome.output
            );
            assert!(
                carrier
                    .asked
                    .lock()
                    .expect("nothing else holds this")
                    .is_empty(),
                "{elsewhere} reached the carrier, which should never have been asked"
            );
        }
    }

    /// **Requests are counted, because fuel does not count them.**
    ///
    /// A host call spends almost no fuel — the waiting happens outside the
    /// interpreter — so without a separate budget an application could sit in a
    /// loop making requests for as long as the other end kept answering, having
    /// spent nearly none of its allowance.
    #[test]
    fn a_run_cannot_make_more_requests_than_it_was_allowed() {
        let wasm = module(&format!(
            r#"(module
                 (import "wasi_snapshot_preview1" "fd_write"
                   (func $write (param i32 i32 i32 i32) (result i32)))
                 (import "ephemeral" "send" (func $send (param i32 i32) (result i32)))
                 (import "ephemeral" "recv" (func $recv (param i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (data (i32.const 1024) "{escaped}")
                 (func (export "_start")
                   (local $left i32)
                   (local $n i32)
                   (local.set $left (i32.const 10))
                   (loop $again
                     (drop (call $send (i32.const 1024) (i32.const {length})))
                     (local.set $n (call $recv (i32.const 2048) (i32.const 8192)))
                     (local.set $left (i32.sub (local.get $left) (i32.const 1)))
                     (br_if $again (local.get $left)))
                   (i32.store (i32.const 0) (i32.const 2048))
                   (i32.store (i32.const 4) (local.get $n))
                   (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1)
                                      (i32.const 8)))))"#,
            escaped = quoted(r#"{"method":"GET","url":"https://api.example.com/ping"}"#),
            length = r#"{"method":"GET","url":"https://api.example.com/ping"}"#.len(),
        ));

        let carrier = Answers::default();
        let outcome = run(
            &wasm,
            &Capabilities {
                requests: 3,
                ..reaching("api.example.com")
            },
            Some(&carrier),
        )
        .expect("running out of budget is an answer");

        assert_eq!(
            carrier.asked.lock().expect("nothing else holds this").len(),
            3,
            "three were allowed, ten were attempted"
        );
        assert!(
            outcome.output.contains("every request it was allowed"),
            "and the application is told, rather than hanging: {}",
            outcome.output
        );
    }

    /// A module that does nothing runs, and is not mistaken for a failure.
    #[test]
    fn a_module_that_does_nothing_succeeds() {
        let wasm = module(r#"(module (func (export "_start")))"#);

        let outcome = run(&wasm, &granted(1_000_000), None).expect("it runs");

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

        let refused =
            run(&wasm, &granted(1_000_000), None).expect_err("it must not be allowed to start");

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

        let refused =
            run(&wasm, &granted(1_000_000), None).expect_err("there is no socket to open");

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

        let stopped = run(&wasm, &granted(100_000), None).expect("being stopped is an outcome");

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

        let stopped = run(&wasm, &granted(200_000), None).expect("being stopped is an outcome");

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
            None,
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
            None,
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

        let outcome = run(&wasm, &granted(1_000_000), None).expect("it runs");

        assert_eq!(outcome.output, "hi\n");
    }

    /// A module that crashes is a crash, not a bound being hit, and not a
    /// success. An unrecognised stop has to land here rather than anywhere
    /// more comfortable, because the comfortable answers are all wrong.
    #[test]
    fn a_module_that_crashes_says_so() {
        let wasm = module(r#"(module (func (export "_start") (unreachable)))"#);

        let crashed = run(&wasm, &granted(1_000_000), None).expect("a crash is an outcome");

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
        assert!(capabilities.writable.is_empty());
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
            capabilities.writable.is_empty(),
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
