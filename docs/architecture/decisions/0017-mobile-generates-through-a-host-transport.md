# ADR-0017: A phone generates for itself, through a C ABI and the host's own HTTPS

- **Status:** accepted
- **Date:** 2026-08-16
- **Deciders:** Ephemeral maintainers
- **Phase:** 5 — Mobile

## Context

[ADR-0007](0007-mobile-control-plane.md) said mobile reaches a control plane
because a phone cannot run a container. That is true, and it was written as
though it settled the whole question. It did not: it conflated *generating* an
application with *running* one, and only the second needs a sandbox.

[ADR-0016](0016-real-providers-live-in-their-own-crates.md) then made the
conflation load-bearing without anyone noticing. A provider that reaches the
network by invoking `curl` cannot work on iOS, where an application may not
spawn a process. So the only real provider Ephemeral had could not run on half
the platforms it claims to target, and the reason was a transport detail rather
than anything about phones.

The consequence was quietly large. Generating is the moment Ephemeral is worth
having — a sentence becomes an application. Routing that through a server would
mean a phone cannot produce software without someone else's machine agreeing,
and would put every user's intent through an intermediary that does not need to
exist.

## Decision

**Three things.**

**Transport is a trait, not a subprocess.** `Transport` in
`ephemeral-provider-anthropic` has one method: given an endpoint, a credential
and a request body, return a response body. `curl` becomes one implementation
behind a default-on `curl` feature, and the crate compiles with that feature
off — which CI checks, because a guarantee nobody checks is a hope.

**Mobile reaches Ephemeral through a C ABI**, in `ephemeral-ffi`. Swift and
Kotlin both speak C; a C ABI is therefore one boundary rather than two, and it
is a boundary that can be compiled against and tested from C on any machine.
The handle is opaque, every returned string is owned by the caller and released
through one function, and no Rust panic crosses the boundary.

**The host supplies the transport.** The app passes two function pointers —
send and free — and Ephemeral calls them for every request. On iOS that is
`URLSession`; on Android whatever the app already uses. Ephemeral opens no
sockets on a phone and brings no HTTP stack, so TLS policy, certificate
pinning, background transfer and proxy behaviour stay with the platform code
that is allowed to have opinions about them.

**What a phone still does not do is run generated code.** `ephemeral_generate`
plans, generates and writes source to the device, and deliberately stops there:
building and running need a sandbox no phone has, and running generated code
outside one is the thing Ephemeral exists to prevent. The application is left
generated-and-unbuilt, which the lifecycle already models. A machine that can
build finishes it — and *that* is what ADR-0007's control plane is for.

## Alternatives considered

### Generate on a server, as ADR-0007 implied

Simple, and it is what most products do. Rejected: it makes the core act of
using Ephemeral conditional on infrastructure existing, funded and up. It also
puts every intent through a third party for a reason that turned out to be a
`curl` invocation rather than a property of phones.

### Bring a Rust HTTP client for mobile only

`reqwest` or `ureq` compiled in behind a `cfg`. Rejected on ADR-0016's grounds,
which do not weaken on a phone: an async runtime and a second TLS stack inside
an app that already has one, and a certificate policy Ephemeral would then own
on a platform where the OS is better at it. The host already has a battle-tested
HTTPS client; asking for it is one function pointer.

### Generate a header with `cbindgen`

Rejected, mildly. The header is the document a platform developer reads, and a
generated one says what the types are but not what the contract is — who owns
which allocation, what is safe to call concurrently, what a null return means.
The header here is hand-written and prose-heavy, and a test compiles a C host
against it so that it cannot drift from the library it describes.

### UniFFI, or one binding per language

More ergonomic per language, and real projects use it happily. Rejected for now
because it is two moving generators to keep working for a surface of ten
functions, and because C is the floor both Swift and Kotlin stand on anyway.
Worth revisiting when the surface is large enough that hand-writing the glue is
the slow part.

## Consequences

### What this makes easier

Generating works on every platform, including the two that could not spawn a
process. The transport seam is testable: the provider's request building, its
retries and its error mapping are now driven by fake transports in CI, where
before the whole path was untested-by-construction. And a desktop that wants a
different HTTP story — a proxy, a corporate CA, a recorded session — implements
one trait.

### What this makes harder

Two transports to keep honest instead of one. A C ABI is a compatibility
surface: changing an exported function is a breaking change for anything already
shipped, in a way a Rust signature is not. Memory ownership is now a contract
in prose rather than a borrow checker, on the host's side of the line.

### What we are accepting

**A phone can generate but not run.** An application created on a phone is real
source on the device that nothing there can execute. That is a deliberate stop,
not an oversight, and the interface has to say so rather than leaving someone
waiting for a build that will not happen.

**The host's transport is trusted to be HTTPS.** Ephemeral hands a credential
and a body to a function pointer and cannot verify what it does with them. On a
phone that function is in the same application binary, so this grants nothing
that was not already granted; it is stated because it is a real difference from
the desktop, where Ephemeral chooses the client.

## Security implications

- The credential never comes from an environment variable on mobile. It is
  passed in explicitly, from Keychain or Keystore, and lives in memory for the
  duration of a call. The FFI does not look for an environment variable at all.
- Generating grants no permission. `ephemeral_generate` records what an
  application asked for; only an explicit decision grants anything, and a
  capability nobody requested cannot be granted through the ABI at all — it is
  refused rather than composed from a string the host supplies.
- No Rust panic unwinds into a Swift or Kotlin frame. Every entry point catches,
  returns a code or null, and leaves a human-readable reason to be fetched.
- The two permission systems stay separate across the boundary
  ([ADR-0003](0003-two-tier-permission-model.md)): the ABI exposes no way to
  reach meta-permissions, and the ledger refuses them independently.

## Revisit when

- The exported surface grows past what is pleasant to hand-write, at which point
  UniFFI earns its keep.
- Streaming a generation matters on mobile, which the one-shot send/return shape
  cannot express.
- A sandbox exists that a phone can actually run, which would make the stop
  after generation unnecessary rather than principled.
