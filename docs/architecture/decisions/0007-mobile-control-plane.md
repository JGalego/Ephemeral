# ADR-0007: Mobile executes through a control plane, and says so

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation (protocol seam), 5 — Cross-platform (implementation)

## Context

Ephemeral's core loop is: generate code, build it, run it in a sandbox. On iOS
and Android, that loop is not available.

iOS forbids downloading and executing new code, forbids JIT for third-party
apps, has no container runtime, and terminates background work aggressively.
Android is more permissive — you can ship an interpreter — but has no container
runtime either, no general build toolchain, and battery and background
constraints that make sustained local builds impractical.

Pretending otherwise produces a design that cannot ship. But the user experience
must not degrade: on a phone, "build me something that compares these CSVs"
should still work.

There is also a privacy fact here that must not be buried. If the app runs on a
server, the user's data goes to a server. That is not an implementation detail
to abstract away — it is the single most important thing a user should know
before handing over a file.

## Decision

**Mobile Ephemeral is a client of a control plane, and the UI is explicit about
where execution happens.**

```text
Mobile Ephemeral ──▶ Ephemeral Control Plane ──▶ Sandboxed Runtime ──▶ Generated App
```

- The mobile app runs the **same Rust core** — manifests, state machine,
  permissions, audit — compiled for iOS and Android. What it does *not* run is
  **execution**.
- **Creating and generating happen on the device.** Describing an application,
  planning it, and asking a model to write it are an HTTPS request and some
  parsing; a phone can do all of it. What iOS forbids is running newly written
  code, which is a different thing.
- Remote execution is a `RemoteRuntime` behind the **same `Runtime` trait**
  desktop uses (ADR-0005). The core is unaware of the difference; the runtime
  seam absorbs it.
- **Execution location is surfaced, not hidden.** Every app displays where it
  runs, and any transfer of user data off the device is an explicit,
  auditable, revocable decision — a permission in its own right, not a
  consequence of installing the app.
- **Permissions remain per-app and default-deny**, evaluated by the same core
  logic, and mapped onto native OS permissions (iOS entitlements, Android
  runtime permissions) for device capabilities the OS owns.
- **The protocol is designed for a local runtime to be added later.** App types
  that can genuinely run on-device — a static web app served into the device's
  webview is the obvious first case — become another `Runtime` implementation,
  with no protocol or core changes.

## Amendment (2026-08-16): the seam is execution, not generation

The first version of this decision said mobile does not run "generation and
execution", as though they were one thing. They are not, and lumping them
together was wrong in a way that quietly cost the product a feature: it implied
a phone could not create an application at all without a server, when in fact
only *running* one needs one.

The mistake had teeth. [ADR-0016](0016-real-providers-live-in-their-own-crates.md)
made the provider reach the network by spawning `curl`, which is fine on a
desktop and impossible on iOS — a process there cannot spawn another process.
So the transport, not the platform, was what actually prevented generating on a
phone, and nothing said so.

The transport is therefore a trait. `curl` is one implementation behind a
default feature; the crate compiles and is tested with no subprocess at all,
and CI builds it that way so that portability is checked rather than claimed.
A mobile build supplies its own HTTPS transport and its own credential — the
`ANTHROPIC_API_KEY` environment variable is a desktop convention, not part of
the design.

What is still remote on mobile is **build, run, and repair**: those need a
sandbox the phone does not have. An application created and generated on a
device is a real, versioned application whose code has not yet been built —
which is a state the lifecycle machine already models.

## Alternatives considered

### Embed an interpreter and run generated apps on-device

Android-only in practice (App Store rules make it non-viable on iOS), and it
would give true local-first mobile. Rejected as a primary model because it
produces two fundamentally different products on two platforms, offers far
weaker isolation than a container, and cannot build anything with native
dependencies. Some form of this may return for a restricted app class.

### On-device WebAssembly sandbox

Permitted on both platforms, capability-based, and genuinely local. The most
promising future direction, and a natural fit for the runtime trait. Rejected
for the MVP for the same reason as ADR-0005: the ecosystem cannot yet run the
general case, and mobile is where we can least afford to support only a narrow
slice of what users ask for.

### Mobile as a pure remote viewer of the user's desktop Ephemeral

Very attractive on privacy — data never leaves the user's own machines — and
worth building as an *option*. Rejected as the only model because it requires
the user to own and keep running a desktop machine, which fails anyone who is
mobile-first.

### No mobile at all

Honest, and cheap. Rejected because the product thesis is that intent is
durable and implementation is disposable, and the device where intent most often
arrives is a phone.

## Consequences

### What this makes easier

A real mobile product without lying about platform capabilities. The core is
shared, so permission and lifecycle semantics cannot drift between phone and
desktop. Cloud execution is introduced behind an existing seam rather than
grafted on.

### What this makes harder

Ephemeral acquires a server-side component, with everything that implies:
operation, availability, multi-tenancy, cost, and a much larger attack surface.
Design principle 10 — no unnecessary cloud dependency — is preserved on desktop
but genuinely relaxed on mobile, and that asymmetry must be stated plainly to
users rather than smoothed over.

### What we are accepting

Mobile is not local-first, and cannot be with today's platform constraints. We
accept it, we disclose it in the UI, and we design so that on-device execution
can be adopted for whatever subset becomes viable without a rewrite.

## Security implications

The control plane is a new trust boundary and a high-value target: it holds
user data in transit, runs untrusted generated code multi-tenantly, and could
observe or tamper with results. It requires its own threat model section,
per-tenant isolation at least as strong as the desktop sandbox, transport
authentication, and an audit trail the user can read from the device. Data
leaving the device is treated as a permission decision, so it appears in the
audit log like any other.

## Revisit when

- On-device Wasm can run a meaningful class of generated apps.
- A desktop-as-your-own-control-plane mode is built (it should be, as the
  privacy-preserving option).
