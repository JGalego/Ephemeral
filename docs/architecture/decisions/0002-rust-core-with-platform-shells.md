# ADR-0002: A Rust core with thin platform shells

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation

## Context

Ephemeral must run on macOS, Windows, Linux, iOS and Android, and must expose a
desktop UI, a mobile UI and a CLI. The parts that must not diverge between those
targets are precisely the parts that decide security outcomes: the permission
model, the lifecycle state machine, the manifest schema, redaction, and the
enforcement points that stand between generated code and the host.

Two properties dominate the choice:

1. **One implementation of the security-critical logic.** Five reimplementations
   of a permission check is five chances to get it wrong, and the one that is
   wrong will be the one nobody audited. Whatever we choose must let the same
   compiled logic serve all five platforms.
2. **Honest platform integration.** Permissions, keychains, process execution
   and notifications are genuinely different per OS, and the good versions of
   them are native. A framework that flattens those differences produces a
   parallel, fake security model — the exact thing SECURITY.md forbids.

Ephemeral also spawns processes, drives a container runtime, and handles
untrusted code and secrets. Memory-safety in that layer is not a nicety.

## Decision

**A Rust core, compiled natively for every target, with thin platform shells.**

- `ephemeral-core` holds the domain: manifests, the state machine, both
  permission systems, audit, retention, storage traits. It performs no host
  I/O, which makes it portable by construction and testable without Docker.
- Platform-specific behaviour sits behind a `PlatformAdapter` trait implemented
  per OS.
- **Tauri v2** is the desktop and mobile shell. It links the Rust core
  in-process, produces small native binaries, uses the system webview rather
  than bundling a browser engine, and — decisively — targets iOS and Android
  from the same Rust core in v2.
- The UI is TypeScript/web, shared across desktop and mobile, with native
  adapters where the platform genuinely does it better.
- The CLI is a Rust binary consuming the same core, not a wrapper around the
  desktop app.

## Alternatives considered

### Electron + Node/TypeScript core, React Native for mobile

Fastest to build a good-looking desktop app, largest ecosystem, and the team-
familiar path. Rejected on three counts. First, the core would be shared with
mobile only by rewriting it for React Native or accepting a second
implementation — precisely the divergence risk we are trying to eliminate.
Second, Electron ships a full Chromium per app: ~150MB for a product whose
pitch is disposability. Third, and worst, the natural Electron architecture puts
Node's full host authority one IPC hop from renderer code — for a product that
handles untrusted generated code, the default is the wrong shape and we would be
fighting it forever.

### Go core with Wails (desktop) and gomobile (mobile)

Excellent process and container ergonomics — the Docker ecosystem is written in
Go — fast builds, easy cross-compilation. Genuinely close. Rejected because
mobile support via gomobile is a binding layer rather than an application
framework, so the mobile client would need a separate native UI on each
platform, and because Go's type system expresses the state machine and
permission algebra less precisely: exhaustive enum matching and `Result` are
what make an illegal transition a compile error here rather than a runtime
branch someone forgot.

### Flutter with a Dart core

The best single-codebase story for UI across all five platforms, and mature.
Rejected because the systems layer — Docker, process supervision, sandboxing,
keychains — would be FFI to native code anyway, so the "one codebase" benefit
does not reach the part of the system that is hard. It would also make Dart the
language of the security-critical core, which we are less willing to defend for
this workload than Rust.

### Native app per platform, shared core via a C ABI / UniFFI

Best possible platform integration and the most native feel. Rejected for Phase
0 as a cost problem, not a correctness one: five UI codebases before the core
loop is proven is exactly the speculative framework-building the methodology
warns against. Note that our chosen structure does not foreclose this — the core
is already behind a clean seam, so a native shell can be added later for any
platform that warrants it.

## Consequences

### What this makes easier

The security-critical logic exists once and is tested once. A permission check
behaves identically on Windows and Android because it is the same machine code
path. Desktop binaries are small. The CLI and UI cannot drift, because they are
clients of the same API rather than of each other.

### What this makes harder

Rust has a steeper contribution curve than TypeScript, which will narrow the
contributor pool. Tauri v2's mobile targets are younger than its desktop
targets, and we should expect rough edges. Cross-compiling five targets in CI is
more setup than a single-runtime project needs.

### What we are accepting

We are betting on Tauri v2 for mobile before its mobile story is as proven as
its desktop story. The bet is bounded: Tauri is the *shell*. If v2 mobile does
not hold up, we replace the shell and keep the core, because the core has no
Tauri dependency. That containment is the point of the seam.

## Security implications

Strongly positive.

- One implementation of permission enforcement, lifecycle rules and redaction —
  one thing to audit rather than five.
- `ephemeral-core` cannot perform host I/O, so it cannot be the site of a
  sandbox escape. Everything dangerous is confined to the adapter and runtime
  crates, which is a much smaller audit surface.
- Memory safety in the layer that supervises untrusted processes and handles
  secrets.
- No bundled browser engine to keep patched, and no architecture that places
  full host authority one hop from web content.

## Revisit when

- Tauri v2 mobile proves unable to deliver the mobile client (→ replace the
  shell, keep the core).
- A platform we must support has no viable Rust toolchain.
- Contributor throughput becomes the binding constraint on the project and is
  attributable to the language choice.
