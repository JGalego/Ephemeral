# ADR-0015: Defer the native runtime rather than ship a weak one

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 1 — Local runtime

## Context

[ADR-0005](0005-docker-first-runtime-abstraction.md) established a `Runtime`
trait with three implementations in mind: `DockerRuntime` for the desktop,
`RemoteRuntime` for mobile, and `NativeRuntime` for the cases that genuinely
cannot be containerised. `RuntimeKind::Native` exists in the manifest, is
documented as materially less isolated, and describes itself that way in the
interface.

`DockerRuntime` is now built. `NativeRuntime` is the remaining Phase 1 item, and
building it turns out to require a decision this ADR exists to record.

The confinement `DockerRuntime` applies is not incidental — it is the product's
central claim. Every capability dropped, a read-only root, no network by
default, a non-root user, a memory ceiling that is not silently a swap ceiling,
a process limit that bounds a fork bomb. `docs/sandbox.md` states each of those
to users as a fact.

A native process on Linux would need `setrlimit`, `seccomp`, namespaces and
cgroups to approach any of it. On macOS it would need the sandbox framework; on
Windows, job objects and AppContainer. This crate **forbids unsafe code**, and
reaching any of those APIs means either a libc binding or a sandboxing crate —
new dependencies in the one crate whose dependency tree is part of the trust
base, which is precisely what [ADR-0014](0014-drive-docker-through-its-cli.md)
declined to do for a much smaller benefit.

## Decision

**Do not build `NativeRuntime` yet.** `RuntimeKind::Native` stays in the
manifest as a modelled possibility; nothing implements it, and an application
declaring it is refused with an explanation rather than run.

The reasoning is not that it is hard. It is that the plausible version — spawn
the process, apply whatever limits are reachable without new dependencies,
label it "less isolated" — would be a sandbox in name only. It would satisfy the
type system, appear in the interface beside the container runtime, and confine
almost nothing.

Ephemeral's whole posture is that generated code is untrusted. A runtime that
cannot enforce the limits its manifest declares is the same failure as an egress
allow-list Docker cannot apply, and it gets the same answer: **refuse, and say
what it would take.**

## Alternatives considered

### Ship it, labelled as weaker

The label already exists, and `RuntimeKind::Native.describe_isolation()` says
"less isolated than usual" today. Rejected because the label is doing more work
than a label can. A user choosing between "runs in a container" and "runs
directly, which is less isolated" is not being asked a question they can answer;
they will pick whichever one works. The honest version of that sentence is
"unconfined", and if that is what it says, nobody should pick it — at which
point it need not exist.

### Ship it for trusted apps only, gated by a meta-permission

Better, because it makes the choice explicit and auditable. Rejected for now
because it inverts the trust model: the whole point is that Ephemeral does not
need to trust a generated application, and "this one is fine" is a judgement
neither the user nor the model is positioned to make about code nobody read.

### Take the dependency and build it properly

The right answer eventually. Deferred rather than rejected: it is a substantial
piece of platform-specific security engineering, it needs its own threat
analysis, and there is currently **no application that requires it**. Building a
security-critical component with no user is how it ends up untested and wrong.

## Consequences

### What this makes easier

The trust base stays small, and there is exactly one implementation of the
confinement that `docs/sandbox.md` describes — so the document cannot be true of
one runtime and false of another. Phase 1's remaining work is honest: what
exists is finished, rather than three-quarters of something.

### What this makes harder

Anything that genuinely cannot be containerised cannot run. Today that is
nothing, because nothing is generated yet; the constraint becomes real when a
generated application needs GPU access, a host device, or a desktop GUI toolkit.

### What we are accepting

`RuntimeKind::Native` is a modelled state with no implementation, which is a
mild dishonesty in the type system. It is kept rather than removed because
deleting it would lose the analysis in ADR-0005 about *why* a weaker tier might
be needed, and the manifest schema is versioned precisely so that a variant can
gain an implementation later without a migration.

## Revisit when

- A generated application actually needs something a container cannot give it,
  and the need is concrete rather than anticipated.
- A sandboxing crate exists that is small enough to audit, cross-platform, and
  does not require this crate to permit unsafe code.
