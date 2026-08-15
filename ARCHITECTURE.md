# Ephemeral — Architecture

> This document describes how Ephemeral is put together and why. It is the
> normative reference for the codebase. Where a decision has meaningful
> trade-offs, it is recorded as an ADR in
> [`docs/architecture/decisions/`](docs/architecture/decisions/) and linked from
> here.

---

## 1. The shape of the system

```text
User
  │
  ▼
Ephemeral UI  (desktop shell · mobile client · CLI)
  │
  │  Core API  (versioned, transport-neutral — §5)
  ▼
Ephemeral Core
  ├── Intent / Task Manager      turns a request into a tracked unit of work
  ├── App Registry               the set of known applications
  ├── State Machine              the lifecycle of every application
  ├── Permission Manager         meta-permissions and app permissions
  ├── Runtime Manager            selects, provisions and drives runtimes
  ├── Generation / Agent I/F     provider-neutral planning, codegen, repair
  ├── Artifact Manager           source, build output, logs, exports
  ├── Secrets / Credentials Mgr  platform-native secure storage
  ├── Storage                    app records, history, workspaces
  └── Audit Log                  append-only record of security decisions
        │
        ▼
   Platform Adapter
        │
        ├── macOS   ├── Windows   ├── Linux   ├── iOS   └── Android
```

Two rules hold this together, and they are enforced by the crate graph rather
than by convention:

1. **The UI never touches Docker, the filesystem, or OS processes directly.**
   Everything goes through the Core API. A UI bug cannot become a container
   escape.
2. **The core never calls a platform API directly.** It calls a `PlatformAdapter`
   trait. Anything platform-shaped — permission prompts, keychains, process
   spawning, notifications — is behind that seam.

---

## 2. Technology choices

| Layer | Choice | Why |
|-------|--------|-----|
| Core domain, runtime, agent orchestration | **Rust** | One core, compiled natively for all five target platforms. Memory safety matters in a system whose job is running untrusted code. Strong enum/`Result` modelling suits an explicit state machine. |
| Desktop shell | **Tauri v2** | Links the Rust core in-process, ships small native binaries per OS, uses the system webview instead of bundling a browser. |
| Mobile client | **Tauri v2 mobile** (iOS/Android) | Same Rust core compiled for mobile, so the manifest, state machine and permission logic are literally the same code — not a reimplementation that drifts. |
| UI | **TypeScript + web UI**, rendered by the platform shell | One UI codebase across desktop and mobile; native adapters where the platform genuinely does it better. |
| CLI | **Rust**, same core crate | The CLI is not a wrapper around the app; both are clients of the same API. |
| Local persistence | **Files + SQLite** | Boring, local-first, inspectable, no server. |

Full trade-off analysis: [ADR-0002](docs/architecture/decisions/0002-rust-core-with-platform-shells.md).

### Crate layout

```text
crates/
  ephemeral-core/      domain model: manifests, state machine, permissions,
                       audit, retention, storage traits.  No I/O to the OS,
                       no Docker, no network.  Pure, testable, portable.
  ephemeral-runtime/   the sandbox: what confines a generated application, and
                       the Docker implementation of it.  The container spec is
                       data; turning it into a command line is a pure function,
                       so every hardening flag is a test.
  ephemeral-cli/       command-line client of the core API
  (planned)
  ephemeral-agent/     AgentProvider trait + Anthropic/OpenAI/local/mock
  ephemeral-platform/  PlatformAdapter trait + per-OS implementations
  ephemeral-api/       the versioned service layer the UI and CLI consume
apps/
  (planned)
  desktop/             Tauri v2 desktop shell
  mobile/              Tauri v2 mobile client
```

`ephemeral-core` deliberately has no dependency that performs I/O against the
host. That is what makes the security-critical logic — permission decisions,
lifecycle transitions, redaction — unit-testable without a container runtime and
identical on every platform.

---

## 3. The domain model

### 3.1 Applications

An application is described by a **versioned manifest** (`schema_version`,
currently `1`). The manifest is the durable, portable description of the app:
identity, runtime, permissions, resource limits, artifacts, retention policy and
lifecycle state.

See [`docs/manifest.md`](docs/manifest.md) for the full schema and
[ADR-0006](docs/architecture/decisions/0006-versioned-manifest-schema.md) for
the versioning rules.

### 3.2 Principals

Every actor that can hold a permission is a **principal**:

```text
Principal::Ephemeral        the product itself
Principal::App(AppId)       one generated application
Principal::Plugin(PluginId) a future plugin (reserved)
```

Principals are isolated from each other by default. App A holds no permission
over app B's data, and no principal inherits another's grants — including the
generated app not inheriting Ephemeral's. This is the single most important
invariant in the system and it is covered by dedicated security tests.

### 3.3 Actors

Distinct from principals, an **actor** is *who caused a thing to happen*, and it
is recorded on every lifecycle transition and audit entry:

```text
Actor::User        a human decision
Actor::Ephemeral   the product's own orchestration
Actor::Agent       the generation agent
Actor::Runtime     a container/process runtime event
Actor::System      the OS, a scheduler, a retention sweep
```

Certain events are restricted to certain actors. The generation agent cannot
grant itself a permission, and it cannot delete an application; only a user can.

### 3.4 The lifecycle state machine

Lifecycle is an explicit, event-driven, deterministic state machine — not an
enum with implicit transitions. `transition(state, event)` is a total function
returning either the new state or a typed error naming the illegal transition.

```text
        REQUESTED
            │ plan
            ▼
        PLANNING ──────────────┐
            │ plan ok          │
            ▼                  │  any transient state may be
        GENERATING             │  interrupted by
            │ generated        │
            ▼                  ├──▶ PERMISSION_REQUIRED ──▶ (resume)
         BUILDING ◀────┐       │
            │ built    │       ├──▶ BLOCKED
            ▼          │       │
        VALIDATING     │       └──▶ CANCELLED
          │      │     │
     pass │      │ fail│
          │      ▼     │
          │  REPAIRING─┘  (bounded iterations)
          ▼
        READY ⇄ RUNNING ⇄ PAUSED
          │        │
          │        └──▶ UNHEALTHY ──▶ RUNTIME_FAILED
          ▼
       ARCHIVED ──restore──▶ READY
          │
          ▼
       DELETED  (tombstone; purge removes the record)
```

Every transition records: previous state, new state, event, actor, reason,
timestamp, metadata and structured error information where applicable. The
history is persisted and is the source of truth for the UI's explanation of what
the system is doing — *"BUILDING because Ephemeral is installing its runtime"*,
not an unexplained spinner.

Full state and transition reference: [`docs/lifecycle.md`](docs/lifecycle.md).
Rationale: [ADR-0004](docs/architecture/decisions/0004-explicit-lifecycle-state-machine.md).

---

## 4. The two permission systems

Ephemeral has two entirely separate permission spaces. Conflating them would be
a security failure, so they are separate types that cannot be substituted for
one another.

### 4.1 Meta-permissions — what *Ephemeral* may do

Installing runtimes, executing processes, using Docker, reading directories,
reaching the network, touching the keychain, camera, microphone, location,
updating itself. Explicit, inspectable, revocable. Never silently escalated.

Where the OS provides a native permission mechanism (macOS TCC, Android runtime
permissions, iOS entitlements), Ephemeral **integrates with it** rather than
building a parallel fake security model. The platform adapter's job is to make
the OS the source of truth and mirror it into the ledger, never to pretend a
grant exists that the OS has not given.

### 4.2 Application permissions — what *one generated app* may do

Each app carries its own permission set, scoped as narrowly as practical:

```yaml
permissions:
  filesystem:
    - read: ~/Downloads/apartments
  network:
    outbound: false
  process:
    execute: false
  camera: false
```

**An app never inherits Ephemeral's permissions.** Ephemeral holding
`filesystem.read(*)` does not let a generated app read anything. A grant to an
app must name the app as its subject and is checked against a default-deny
ledger.

### 4.3 Decisions and the ledger

Every permission decision is a `Grant` — subject principal, permission,
allow/deny, granting actor, timestamp, optional expiry, revocation state —
recorded in an append-only ledger and mirrored into the audit log. Checks are
default-deny; an explicit `Deny` beats an `Allow`; a revoked or expired grant
does not apply.

Rationale: [ADR-0003](docs/architecture/decisions/0003-two-tier-permission-model.md).
Detail: [`docs/permissions.md`](docs/permissions.md).

### 4.4 Permission UX

A permission request is not a dialog string; it is a structured `PermissionPrompt`
carrying the five answers the UI must show:

> **What is asking?** Apartment Comparator (a generated app)
> **What does it want?** Read the files in `~/Downloads/apartments`
> **Why?** To compare the CSV files you selected
> **What happens if you allow?** It can read those files. It still cannot
> reach the network or read anything else.
> **Can you revoke it?** Yes, any time, from the app's detail page.

The core produces those five fields from the permission itself, so no UI can ship
a meaningless *"Allow filesystem access?"* prompt.

---

## 5. The Core API

One versioned, transport-neutral API sits between clients (desktop UI, mobile
client, CLI) and the core. In-process today via Rust traits; the same shapes
serialise for the mobile control plane later without redesign.

```text
Clients ──▶ Core API (v1) ──▶ services ──▶ Runtime / Platform / Agent
```

Rules:

- The API is versioned and documented; breaking changes bump the version.
- Clients hold no privileged handles — no Docker socket, no raw paths.
- Every privileged operation crosses the API and is audited on the far side.

---

## 6. Runtimes

Generated applications run behind a `Runtime` abstraction:

```text
Runtime (trait)
├── DockerRuntime   desktop default; container isolation, resource limits,
│                   explicit mounts, explicit port exposure
├── NativeRuntime   constrained local execution for cases that genuinely
│                   cannot use a container, with tighter permission gates
└── RemoteRuntime   (mobile) sandboxed execution via the control plane
```

Docker is **supported on all three desktop platforms and required for none**.
The runtime manager detects Docker, explains clearly when it is absent, and only
installs it with the corresponding meta-permission. Functionality that can run
safely without Docker must not be gated behind it.

Rationale: [ADR-0005](docs/architecture/decisions/0005-docker-first-runtime-abstraction.md).

---

## 7. Mobile

iOS and Android do not permit arbitrary generated code to be compiled and
executed locally, and pretending otherwise would produce a design that cannot
ship. Mobile Ephemeral is therefore a **client of a control plane**:

```text
Mobile Ephemeral ──▶ Ephemeral Control Plane ──▶ Sandboxed Runtime ──▶ Generated App
```

The user experience is unchanged — *"build me X"* — but the UI is explicit about
where the app executes, because that is a privacy-relevant fact, not an
implementation detail to hide. Locally-runnable app types (web apps served to
the device's webview, for instance) may later run on-device without protocol
changes: the runtime seam is the same one desktop uses.

Rationale: [ADR-0007](docs/architecture/decisions/0007-mobile-control-plane.md).

---

## 8. Generation

The agent layer is provider-neutral behind an `AgentProvider` trait — Anthropic,
OpenAI, a local model, or a deterministic mock. **CI never depends on a live LLM
call**; the mock provider produces fixed, reproducible outputs so end-to-end
tests are deterministic.

The build/repair loop is bounded on every axis — iterations, wall-clock, CPU,
memory, artifact size, network, and spend — and the user can cancel it at any
point.

```text
PLAN → GENERATE → BUILD → TEST → INSPECT ──pass──▶ READY
                    ▲                  └─fail─▶ DIAGNOSE → REPAIR ─┘
```

Generated code never executes with unrestricted host privileges, and model
output is treated as untrusted input to the system, not as instructions to it.

Rationale: [ADR-0008](docs/architecture/decisions/0008-agent-provider-abstraction.md).

---

## 9. Storage, ephemeralness and retention

Every app gets a predictable, separated storage hierarchy:

```text
<data-root>/apps/<app-id>/
  source/      generated source
  build/       build output
  runtime/     runtime scratch, destroyed on teardown
  data/        the app's persistent data
  logs/        build, test and runtime logs
  artifacts/   exports and reports
```

Secrets are *not* in that tree. They live in platform-native secure storage and
are injected into runtimes as values the manifest and UI never see.

**Ephemeralness is a first-class property**, not a cleanup afterthought. Each app
carries a retention policy:

| Policy | Behaviour |
|--------|-----------|
| `one-shot` | created, run, deleted |
| `ephemeral` | expires quickly (default 24h) |
| `temporary` | remains dormant, expires (default 7d) |
| `reusable` | available until explicitly archived |
| `persistent` | behaves like a conventional application |

The user can change an app's policy at any time. Deletion is recoverable through
an archive/trash period unless the user explicitly chooses to purge.

Rationale: [ADR-0009](docs/architecture/decisions/0009-storage-layout-and-retention.md).

---

## 10. Observability and audit

Two distinct things, for two distinct audiences.

**Observability** is for understanding the app: lifecycle history, build history,
test results, logs, runtime health, resource usage, repair attempts. A user sees
*"Ephemeral rebuilt the app after a failed test."* A developer sees
*build #17 → test failure → repair → build #18 → passed.*

**The audit log** is for security. Append-only, hash-chained so tampering is
detectable, and covering every security-sensitive operation: permission requests
and decisions, container creation, mount and port exposure, secret access
(never secret *values*), deletion and purge. Redaction runs on the way in, so
secrets cannot be written to the log in the first place.

Rationale: [ADR-0010](docs/architecture/decisions/0010-hash-chained-audit-log.md).

---

## 11. Sandboxing

Generated code is untrusted. Defence in depth, with no single control load-bearing:

- container isolation by default on desktop; process isolation otherwise
- non-root execution, dropped capabilities, no privilege escalation
- explicit, minimal host mounts — **never** the user's home directory
- network denied by default; egress allow-listed when granted
- CPU, memory, storage, PID and wall-clock limits
- secret isolation — Ephemeral's own credentials are never visible to a
  generated app
- temporary workspaces destroyed on teardown, with orphan cleanup
- explicit port exposure, bound to loopback unless the user says otherwise

---

## 12. App-to-app composition

Generated apps will eventually compose (research → clean → visualise → report).
Composition is via **explicit capability contracts** between isolated
principals — never ambient access to another app's files or endpoints. The design
space is reserved; the MVP does not implement it.

---

## 13. Local-first

Desktop Ephemeral works without a cloud dependency: local state, local
execution, local Docker, local source, local logs. Only the generation provider
needs the network, unless a local model is configured — and the UI distinguishes
*"Ephemeral is offline"* from *"this generated app needs network access"*,
because they are different problems with different fixes.

Cloud sync and remote execution are additive, behind the same seams the mobile
control plane uses.

---

## 14. What exists today

Phase 0 is the foundation: this document, the ADRs, CI, and `ephemeral-core` —
the manifest, the lifecycle state machine, the two permission systems, the audit
log, retention policy and the storage layout — plus a CLI that exercises them.
Runtimes, generation and the desktop UI arrive in Phases 1–4. See
[`docs/roadmap.md`](docs/roadmap.md).
