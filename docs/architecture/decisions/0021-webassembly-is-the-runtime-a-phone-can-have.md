# ADR-0021: WebAssembly is the runtime a phone can have

- **Status:** accepted
- **Date:** 2026-08-23
- **Deciders:** Ephemeral maintainers
- **Phase:** 1 — Local runtime

## Context

[ADR-0005](0005-docker-first-runtime-abstraction.md) named three runtimes:
`DockerRuntime` for the desktop, `RemoteRuntime` for mobile, `NativeRuntime` for
whatever cannot be containerised. `DockerRuntime` was built.
[ADR-0015](0015-defer-the-native-runtime.md) declined to build `NativeRuntime`,
on grounds that have not weakened: a native process starts with the whole
machine, confinement means subtracting from it one platform-specific mechanism
at a time, and the plausible version — spawn it, apply what is reachable, label
it "less isolated" — is a sandbox in name only.

That left mobile with `RemoteRuntime`, which is not a local runtime at all. A
phone could describe an application, generate one, hold its manifest, decide its
permissions and show its version history. It could not run it. Everything the
product is *for* happened on somebody else's computer, and the honest sentence
in the interface was that the user's data leaves the device.

Asked to accept that, the owner declined: *this absolutely cannot be.* Correctly.
"A phone cannot run untrusted code" is false — every phone runs untrusted code
all day, in a browser tab, through an execution model built for exactly this.
The premise that needed re-examining was not whether a handset can confine
something. It was whether confinement has to be built by subtraction.

## Decision

**Add a fourth runtime, and implement it: WebAssembly, interpreted, in
Ephemeral's own process.** `RuntimeKind::Wasm`, `crates/ephemeral-runtime/src/wasm`,
behind a `wasm` feature that pulls in an interpreter and nothing else.

**It is not the weak runtime ADR-0015 refused, and the difference is the whole
argument.** A WebAssembly module starts with *nothing*. It has no syscalls. It
cannot name a file, open a socket, read the clock, or learn its own process id
unless the host hands it a function that does so. Confinement is not applied to
it; confinement is its resting state. Every capability is an explicit addition,
so a control this crate forgets to add is a control the application does not
get — the same property `ContainerSpec::minimal` is built around, enforced here
by the execution model instead of by remembering.

That inverts ADR-0015's objection rather than arguing with it. ADR-0015 is about
what happens when a sandbox is a list of removals and the list is incomplete.
Here there is no list.

**Four bounds, each with a test that tries to break it.**

| Bound | Mechanism | The test |
|---|---|---|
| Filesystem | WASI preopens; a descriptor can only be derived from one already held | `a_granted_folder_is_the_only_one_it_can_name` |
| Network | no sockets exist in this WASI implementation to import | `there_is_no_socket_to_open` |
| Processing | fuel, counted in instructions | `a_module_that_loops_forever_is_stopped` |
| Anything else | an unresolved import means the module never instantiates | `a_module_asking_for_what_it_was_not_given_never_starts` |

The fourth row is the one that matters most. It is not a check Ephemeral
performs. A module that imports a function nothing provides has nothing to bind
to, so an application that wants a socket fails at the door rather than halfway
through doing something.

**Interpreted, deliberately, not compiled.** iOS does not permit an application
to generate and execute machine code, which rules out every just-in-time engine
including wasmtime. `wasmi` is pure Rust and interprets. That costs speed and
buys every platform, and for what Ephemeral generates — read a file, count
something, print an answer — it is the right side of the trade. A runtime that
exists on one platform and not the others is how mobile ended up with nothing.

**Fuel, not a wall clock.** Instructions cannot be escaped by sleeping, blocking
or being descheduled, and the bound is the same on a fast phone and a slow one.
A timeout in seconds means something different on every device it runs on, which
is not a bound so much as a lottery.

**Networking has no representation, rather than a grant that quietly does
nothing.** This runtime is *stricter* than the container one in exactly one
place: `Egress::Allowed` cannot be honoured, because there is no socket to
offer. The translation from a specification to capabilities says so out loud
instead of appearing to grant it.

**Refusal is still the rule.** A module that cannot be given what it was
promised is not started with less. `WasmError::Ungranted` and
`WasmError::Stopped` are separated from each other on purpose: one is Ephemeral
refusing and the other is an application misbehaving, and presenting a refusal
as a crash would teach somebody to distrust the sandbox.

## Consequences

**A phone gets a real local runtime, and `RemoteRuntime` stops being the only
answer.** `RuntimeKind::Wasm.runs_locally()` is true and
`describe_isolation()` says so without a hedge, because for once there is
nothing to hedge about.

**The application has to *be* WebAssembly.** This is the real cost, and it is
not paid by this ADR. What it means for something a model wrote in Python is a
question for the layer above; what this decision owes that layer is a place to
run where the permission model is real rather than aspirational.

**Two runtimes now confine, by different mechanisms.** Docker stays the desktop
default: it is faster, it runs anything with an image, and its confinement is
the one `docs/sandbox.md` documents. WebAssembly is what exists where Docker
does not, and what a `Job`-shaped application can use anywhere.

**One more dependency in the trust base.** `wasmi` and `wasmi_wasi`, behind a
feature flag, in the crate whose dependency tree is part of the trust base —
which is what [ADR-0014](0014-drive-docker-through-its-cli.md) declined to do
for a much smaller benefit. The benefit here is a runtime where there was none,
the crate is pure Rust with no unsafe in this crate's own code, and the feature
is off in the builds that do not need it.

**Pinned to `wasmi` 0.46, not 1.x.** 1.x requires Rust 1.86 and this workspace
declares 1.85. Moving a published minimum is the owner's decision, not a
dependency's.

## Alternatives considered

### Keep mobile on the remote runtime

What the code did. Rejected: it makes the product's central act — running the
thing you asked for — the one act that requires trusting a server, on the
platform where people keep the most personal data.

### Build `NativeRuntime` after all, for mobile only

Both mobile platforms forbid spawning processes from a sandboxed application, so
there is not even a weak version of this to ship. It is not a worse answer than
WebAssembly; it is not an answer.

### An embedded interpreter for one language, with no sandbox layer

Ship a Python or JavaScript interpreter and run the model's source in it
directly. Rejected: the sandbox would then be that interpreter's own — which for
CPython is nothing at all, and for a JavaScript engine is a boundary designed to
protect a page rather than a filesystem. It also picks the language for every
application forever. WebAssembly is a target, so the language question stays
open and stays above the runtime.

### wasmtime, and skip iOS

Faster by a wide margin, and the mainstream choice. Rejected because it cannot
ship on iOS at all, and a runtime that exists on some platforms is how this
problem started. The performance is worth revisiting per-platform later; the
portability is not worth losing now.
