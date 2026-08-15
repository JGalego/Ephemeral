# Threat model

What Ephemeral is defending against, what it actually does about each thing,
and — the part that matters most — what it does **not** defend against.

This document is deliberately unflattering. A threat model that concludes the
system is safe is a marketing document. The value is in the rows marked
*accepted* and *not mitigated*, because those are the ones somebody deciding
whether to trust this needs to read.

Status: **first complete pass.** Every mitigation marked ✅ is implemented and
has a test; ⚠️ is partial; ❌ is designed but not built. Anything that changes
this document should change the tests alongside it.

## The shape of the problem

Ephemeral is unusual among desktop applications in that **it runs code nobody
read**. A model writes it, and it executes on a person's machine, near their
files. That single fact generates most of what follows.

Three consequences shape everything:

1. **Generated code is hostile until confined.** Not "probably fine" — the
   design must hold even if the model was steered into writing something
   deliberately malicious.
2. **The model can be steered by data.** Anything the agent reads — a filename,
   a CSV cell, a build error, another person's chat message — is attacker-
   controlled input in the worst case.
3. **The user cannot audit the code.** "Review the source before running it" is
   not available as a mitigation, because the whole product exists to spare
   people that. The manifest and the sandbox have to carry that weight instead.

## Who might attack

| Adversary | What they want | Can they reach us? |
|---|---|---|
| **A steered model** | Whatever the steering says: exfiltration, persistence, lateral movement | Yes — via any content the agent reads |
| **A malicious dependency** | Execution during build or run | Yes — generated code names its own dependencies |
| **A malicious application author** | A recipient to build and run their code | Yes, once sharing exists ([ADR-0012](../architecture/decisions/0012-sharing-distributes-recipes.md)) |
| **A local process** | Ephemeral's stored secrets, or its audit record | Yes — same user account, same machine |
| **A compromised model provider** | To influence what gets generated, or read what is sent | Yes, by construction |
| **A malicious session participant** | Other participants' data, or to steer their agents | Once sharing exists ([ADR-0013](../architecture/decisions/0013-how-several-people-share-an-application.md)) |
| **A network attacker** | Traffic between Ephemeral and a provider or relay | Yes |

Explicitly **out of scope**: an attacker who already has root on the machine, a
malicious operating system, hardware implants, and evil-maid attacks. Nothing in
a userspace application meaningfully defends against those, and claiming
otherwise would be dishonest.

## Threats

### T1 — Generated code does something malicious

The central threat. Everything else is a variation.

| | |
|---|---|
| **Mitigation** | The container sandbox: all capabilities dropped, `no-new-privileges`, read-only root, non-root user, `--network none` by default, mounts read-only unless write was granted, `--pids-limit`, memory ceiling with swap pinned so it cannot be evaded ([docs/sandbox.md](../sandbox.md)) |
| **Status** | ✅ Implemented, asserted by pure-function tests that run without a daemon |
| **Residual risk** | A container escape defeats all of it. Containers are a strong boundary, not a perfect one. |

**Not mitigated:** a kernel or runtime vulnerability that allows escape. The
honest position is that Ephemeral's isolation is exactly as good as the
container runtime's, and a VM-backed runtime would be stronger. That is recorded
as a revisit condition in [ADR-0005](../architecture/decisions/0005-docker-first-runtime-abstraction.md),
not solved.

### T2 — Prompt injection steers the agent

Any content the agent reads may be adversarial: a filename, a cell in a
spreadsheet, a build error from a dependency, a web page, a message from another
participant.

| | |
|---|---|
| **Mitigation** | The agent is **not a privileged actor**. It cannot grant a permission, widen a limit, delete an application, or raise the lifecycle events reserved to a person. Its output is typed data with nowhere to express such a thing — enforced in the core, never in a prompt ([ADR-0008](../architecture/decisions/0008-agent-provider-abstraction.md)) |
| **Status** | ✅ Implemented. `AgentProvider` returns proposals only; the security suite asserts the agent cannot grant or delete |
| **Residual risk** | A steered agent can still write malicious *code*, and can still request permissions with a persuasive stated reason |

**Not mitigated:** a convincing lie in a permission request. If an application
asks for network access "to check for updates" and the real reason is
exfiltration, nothing here detects that. The defences are that the request is
visible before it is granted, the reason is presented as the *model's claim*
rather than as fact, and the sandbox means denying it is cheap.

**This is the weakest point in the product.** A user who approves everything gets
very little protection, and no amount of interface design fully fixes that.

### T3 — Secret exfiltration

| | |
|---|---|
| **Mitigation** | Secret *values* cannot enter a container specification — the type carries names only, and values are passed through the child process's environment, so they never appear in an argument vector, the process table, an error message or the audit log. `Secrets` has a hand-written `Debug` that prints a count. The audit log redacts on the write path, not the read path |
| **Status** | ✅ Implemented and tested |
| **Residual risk** | An application legitimately granted a secret can do anything with it, including send it somewhere if it also has network access |

**Accepted:** granting a secret to an application that also has network access is
a decision only the user can make, and Ephemeral's job is to make sure they are
making it knowingly rather than to prevent it.

### T4 — Malicious or compromised dependencies in generated code

Generated code names its own dependencies. Those are third-party packages nobody
reviewed, and installation typically executes code.

| | |
|---|---|
| **Mitigation** | Dependencies are resolved and installed **inside the sandbox**, under the application's permissions, never Ephemeral's. The reference application is dependency-free, and a test asserts its build recipe runs no fetcher |
| **Status** | ⚠️ Partial. The confinement is real; there is no allow-list, pinning, or provenance check on what a generated application may depend on |
| **Residual risk** | A build that is permitted network access can fetch anything, and install scripts run inside the container |

**Not mitigated:** typosquatting and malicious packages. A generated application
that needs a dependency needs network access at build time, and once it has that
the registry is trusted. Narrowing this needs an egress proxy (see T9) and a
dependency policy; neither exists.

### T5 — Supply-chain attack on Ephemeral itself

| | |
|---|---|
| **Mitigation** | Committed lockfile treated as authoritative and verified in CI; `cargo-deny` for advisories, licences, sources and duplicates; automated dependency updates; a deliberately small dependency tree in the crate that does the confining ([ADR-0014](../architecture/decisions/0014-drive-docker-through-its-cli.md)) |
| **Status** | ✅ Implemented for the build; ❌ release signing and SBOMs do not exist because there are no releases yet |
| **Residual risk** | A compromised upstream crate that passes advisory checks |

### T6 — A compromised or hostile model provider

The provider sees every intent, and chooses every line of generated code.

| | |
|---|---|
| **Mitigation** | Provider-neutral interface, so a provider can be replaced; a local provider is a supported shape; output is validated as structured data rather than trusted; generated code is confined regardless of who wrote it |
| **Status** | ⚠️ The trait and the mock exist; no real provider is implemented yet, local or remote |
| **Residual risk** | The provider learns what the user asked for. That is inherent to using a hosted model |

**Accepted and stated:** using a hosted provider means the intent leaves the
machine. The mitigation available is choice, not prevention — and the offline
path is a local model, not a promise that a remote one is private.

### T7 — Persistence after deletion

An application that survives being deleted is the product's central promise
broken.

| | |
|---|---|
| **Mitigation** | `--restart no`, so nothing resurrects itself and the state machine stays in charge. Deleting revokes every grant immediately. Purging removes the container, its anonymous volumes and the application's data. `ephemeral cleanup` finds containers no application accounts for, and `doctor` reports them |
| **Status** | ✅ Implemented and tested |
| **Residual risk** | An application with a granted write scope can leave files inside that scope, and those survive — as they should, because they are the user's files in the user's directory |

**Not mitigated:** an application granted write access outside its own storage
can write something that persists. This is not a bug; it is what the grant
means. It is worth stating because "throws it away when you're done" invites the
assumption that nothing survives.

### T8 — Resource exhaustion

| | |
|---|---|
| **Mitigation** | CPU, memory (with swap pinned equal, so the ceiling cannot be evaded), process count, and a wall-clock limit enforced by `ephemeral watch`. Generation is bounded on repairs, wall clock and spend |
| **Status** | ⚠️ Container limits ✅; wall-clock and disk ceilings are enforced by `ephemeral watch`, and nothing starts it automatically |
| **Residual risk** | Both time and disk go unenforced whenever nothing is watching |

**Partially mitigated:** the disk ceiling is measured over the application's own
data directory rather than asked of Docker, because that directory is a host
bind mount and is the thing that actually grows. The walk is bounded, and it
under-reports rather than hangs — erring towards leaving an application running.

**Not mitigated:** an application that fills its disk between sweeps, or while
`ephemeral watch` is not running. A background supervisor would close this; the
hosting decision it needs has not been made.

### T9 — Network egress from generated code

| | |
|---|---|
| **Mitigation** | `--network none` by default. An application that only listens gets an `--internal` network — reachable from the machine, unable to reach off it. Published ports bind to `127.0.0.1` |
| **Status** | ✅ for "none" and "everything"; ❌ for anything in between |
| **Residual risk** | None, because the unenforceable case is refused rather than approximated |

An application granted a **hostname allow-list refuses to start**, because
Docker cannot filter egress by destination and ordinary networking would hand
over the whole internet instead of the four hosts its owner allowed. That is a
functional gap deliberately preferred to a security one.

### T10 — Tampering with Ephemeral's own records

| | |
|---|---|
| **Mitigation** | The audit log is append-only and hash-chained; `doctor` verifies the chain and reports a break as a **security event** rather than corruption. Manifests are written atomically, so a crash cannot leave a half-written record |
| **Status** | ✅ Implemented and tested |
| **Residual risk** | A local process running as the user can delete the whole log |

**Not mitigated:** deletion or wholesale replacement of the audit log by a
process with the user's privileges. Hash-chaining detects *modification*, not
destruction, and defending against a same-user attacker is not something a
userspace application can do. Remote attestation would change this; it is not
planned.

### T11 — A shared session (not built)

Everything in [ADR-0013](../architecture/decisions/0013-how-several-people-share-an-application.md).
Listed here because SECURITY.md says sharing must not be built before this
document exists.

| Concern | Position |
|---|---|
| Malicious relay operator | The group operates its own relay and there is no other kind, so the operator is always somebody who can already read the messages |
| Malicious member | Sees all session content by construction. Stated, not prevented |
| Code delivery to guests | Whoever serves the browser bundle can subvert it. Not solvable in a browser; the tier is labelled weaker |
| Invite leakage | An invite is a capability — scoped, expiring, individually revocable, with revocation effective on a live session |
| Injection via another participant's content | A message is untrusted input that may later reach a repair run. T2 with a cheaper delivery route |

**Status:** ❌ none of it is built. This is a design position, not a defence.

### T12 — Malicious plugins

| | |
|---|---|
| **Status** | ❌ Not built, and not designed beyond `PluginId` existing as a principal |
| **Position** | A plugin is a third principal alongside Ephemeral and applications, and gets its own permission space. Nothing else is decided |

## What we do not defend against

Collected in one place, because scattering them through a document is how they
get missed.

1. **A container escape.** Isolation is exactly as good as the runtime's.
2. **A user who approves everything.** The permission model protects a person
   making decisions; it cannot protect one who is not.
3. **A convincing lie in a permission request.** The reason is the model's claim
   and is presented as such. Nothing verifies it.
4. **An attacker with the user's privileges.** They can read the same files and
   delete the same logs.
5. **A hosted provider learning the user's intent.** Inherent; the answer is a
   local model, not a promise.
6. **Disk and time ceilings when nothing is watching.** Both are enforced by
   `ephemeral watch`, and nothing starts it for you.
7. **Malicious dependencies of generated code.** Confined, not vetted.
8. **Anything at all, once `NativeRuntime` exists.** It does not exist, and
   [ADR-0015](../architecture/decisions/0015-defer-the-native-runtime.md)
   records why shipping a weak one was refused.

## How this stays true

Each ✅ above corresponds to tests that fail if the property stops holding:
`crates/ephemeral-core/tests/security.rs` for the permission and audit
invariants, and the argv tests in `crates/ephemeral-runtime/src/docker/command.rs`
for every confinement flag. A change that weakens one should be treated as a
vulnerability rather than a refactor.

The gaps are tracked in [the roadmap](../roadmap.md). A gap that stops being
listed without being closed is the failure mode this document exists to prevent.
