# ADR-0005: A runtime abstraction, Docker-first on desktop but never required

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation (trait), 1 — Local runtime (implementation)

## Context

Generated code is untrusted, so it needs isolation with real teeth: filesystem
containment, network control, resource limits, clean teardown. It also needs to
work identically enough on macOS, Windows and Linux that a generated app is
reproducible across them.

Docker is the obvious answer and the product brief requires supporting it. But
Docker is a heavy prerequisite: it is not installed on most machines by default,
requires a VM on macOS and Windows, needs elevated privileges to install, and
consumes gigabytes. Making it a hard dependency would mean a first-run
experience of "install Docker Desktop, then come back", for a product whose
pitch is that software should appear when you need it.

The opposite error is equally bad: running generated code directly on the host
because Docker is inconvenient.

## Decision

**A `Runtime` trait, with Docker as the desktop default and a hard dependency of
nothing.**

```text
Runtime (trait)
├── DockerRuntime    desktop default
├── NativeRuntime    constrained local execution, tighter permission gates
└── RemoteRuntime    mobile / control-plane execution
```

The trait covers the full container lifecycle: availability detection, image
pull and build, container create/start/stop/restart/destroy, port exposure,
approved mounts, resource limits, log collection, health inspection and cleanup
of orphaned resources.

The runtime manager's obligations:

- **Detect, don't assume.** Probe for Docker; never assume a socket exists.
- **Explain, don't fail silently.** When Docker is missing, unreachable, or the
  daemon is not running, say which, and say what the user can do about it. This
  is a `doctor` check, not a stack trace.
- **Install only with meta-permission.** Installing a container runtime is a
  privileged act requiring the `docker.install` meta-permission and an explicit
  user decision.
- **Do not gate the ungated.** Anything that can run safely without Docker must
  not require it. Creating, inspecting, archiving, restoring and deleting apps,
  and every static analysis of generated code, work with no container runtime
  present.
- **Select per app, record the choice.** The runtime is part of the manifest, so
  an app's execution environment is a durable, inspectable fact rather than an
  ambient property of the machine.

`NativeRuntime` exists for cases that genuinely cannot be containerised. It
carries weaker isolation, so it demands stricter permission gating and is
labelled as such in the UI — the user is told plainly that an app is running
with less isolation.

## Alternatives considered

### Docker as a hard requirement

Simplest, strongest isolation guarantee, one code path. Rejected because it
makes first run a multi-gigabyte installation with an administrator prompt for a
product about disposable software, and because it would gate features that need
no isolation at all behind a container runtime.

### Podman as the default instead of Docker

Rootless by default and daemonless — genuinely better security defaults, and
rootless containers are a meaningful mitigation for container escape. Rejected
as *default* purely on installed base: on macOS and Windows, Docker Desktop is
what users already have. Podman is API-compatible enough that it becomes a
`DockerRuntime` variant rather than a separate implementation, and we intend to
support it and to prefer rootless mode wherever available.

### A WebAssembly runtime (Wasmtime/WASI) as the primary sandbox

Extremely attractive on security: capability-based by construction, no ambient
authority, tiny startup, trivially embeddable, and identical semantics on every
platform including mobile. Rejected as the *primary* runtime for the MVP because
the ecosystem cannot yet run the general case — arbitrary Python or Node
applications with native dependencies, which is most of what users will ask
for. This is the most likely future addition to the trait, and the trait exists
partly so that adding it is not a rewrite.

### Per-OS native sandboxes (seccomp/namespaces, Sandbox.app, AppContainer)

No prerequisite, tight OS integration, low overhead. Rejected as the primary
mechanism because three separate implementations of a security boundary is three
chances to get it wrong, and because reproducibility across platforms —
something the manifest promises — is very hard to deliver this way. These
mechanisms are instead used to *harden* `NativeRuntime`, as defence in depth
rather than as the boundary itself.

### A microVM runtime (Firecracker, Krun)

Stronger isolation than containers, at meaningful cost in startup time,
platform support and operational complexity. Deferred: a plausible future
`Runtime` implementation for high-risk apps, not an MVP default.

## Consequences

### What this makes easier

Strong isolation where it matters, without a hard prerequisite for the whole
product. New runtimes (Wasm, microVM, remote) become additions behind an
existing trait. The mobile control plane reuses the same seam, so remote
execution is not a special case in the core.

### What this makes harder

Multiple runtimes means multiple isolation stories to document, test and
explain to users. `NativeRuntime` is the weakest link by construction and needs
continuous scrutiny to stop it becoming the convenient default. Docker's
behaviour differs subtly between Desktop-on-macOS, Desktop-on-Windows, WSL2 and
native Linux, and integration tests must cover that matrix.

### What we are accepting

That `NativeRuntime` offers materially weaker isolation than `DockerRuntime`,
and that some users will run there. We mitigate with stricter permission gating,
explicit UI labelling, and OS-native hardening — but we are not pretending the
two are equivalent.

## Security implications

The runtime is the primary enforcement point for the sandbox, so this is one of
the highest-consequence decisions in the system. Non-negotiables that any
`Runtime` implementation must uphold, and that the security test layer verifies:

- never mount the user's home directory; mounts are explicit, minimal and
  granted
- non-root execution, dropped capabilities, no privilege escalation
- network denied by default; egress allow-listed when granted
- CPU, memory, storage, PID and wall-clock limits applied, not merely requested
- ports bound to loopback unless the user decides otherwise
- Ephemeral's own credentials never present in a container's environment
- workspaces destroyed on teardown, orphans reaped

## Revisit when

- The Wasm ecosystem can run mainstream Python/Node workloads with native
  dependencies (→ likely a new primary runtime for a large class of apps).
- Rootless Podman reaches parity in installed base on macOS and Windows (→
  reconsider the default).
