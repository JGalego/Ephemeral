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
  generation and execution.
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
