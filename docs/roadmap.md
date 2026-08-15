# Roadmap

Ephemeral is built in phases. A phase is not finished because code exists for
it — it is finished when the previous phase demonstrably works. This page is the
honest record of where that line currently sits.

## Where we are

**Phase 2 — Generation.** In progress.

### Done

| | |
|---|---|
| Repository, licence, contribution guide, security policy | ✅ |
| [ARCHITECTURE.md](../ARCHITECTURE.md) and fifteen [ADRs](architecture/decisions/) | ✅ |
| CI: format, lint, docs, tests on Linux/macOS/Windows, supply chain | ✅ |
| One-command development bootstrap | ✅ |
| `ephemeral-core`: identity, actors, errors | ✅ |
| Lifecycle state machine — 20 states, 31 events, total and actor-authorised | ✅ |
| Both permission systems and the ledger | ✅ |
| Versioned application manifest | ✅ |
| Retention policies | ✅ |
| Hash-chained audit log with redaction on write | ✅ |
| Storage layout and application store | ✅ |
| Security invariant test suite | ✅ |
| `ephemeral-cli` — the same domain model, driven from a terminal | ✅ |
| `ephemeral doctor` — environment diagnostics | ✅ |
| Reference documentation | ✅ |
| `ephemeral-runtime` — the sandbox, and the Docker implementation of it | ✅ |
| `ephemeral run`, `stop`, `pause`, `resume` | ✅ |
| Orphan cleanup — `ephemeral cleanup`, and `doctor` reporting what is left over | ✅ |
| `ephemeral status` — crash, health and clean-exit detection against the record | ✅ |
| `ephemeral logs` shows the application's own output, not only its history | ✅ |
| `ephemeral watch` — the supervisor: crash detection and wall-clock limits | ✅ |
| Building an image from an application's own source | ✅ |

Phase 0 is complete. Everything that does not need a runtime or a model provider
works end to end: ask for an application, inspect it, move it through its
lifecycle, grant and revoke permissions, read the audit trail, delete it,
restore it, purge it.

```console
$ ephemeral create "compare these two CSV files and show me what's different"
$ ephemeral grant <app> read:'~/Downloads/apartments/**' --why "to compare them"
$ ephemeral inspect <app>
$ ephemeral audit
```

Phase 1 has the sandbox: the `Runtime` trait, the Docker implementation
([ADR-0005](architecture/decisions/0005-docker-first-runtime-abstraction.md),
[ADR-0014](architecture/decisions/0014-drive-docker-through-its-cli.md)),
detection with a remedy, container lifecycle, approved mounts, loopback port
exposure, resource limits, logs, health inspection and teardown.

The confinement is decided by pure functions, so what it holds is a set of
tests rather than a claim: capabilities dropped, no network unless granted,
read grants mounted read-only, ports on 127.0.0.1, non-root, no whole-root
mount, and a refusal rather than a weaker substitute when a control cannot be
applied.

`NativeRuntime` is **deliberately not built** — see
[ADR-0015](architecture/decisions/0015-defer-the-native-runtime.md). The version
that could be built without new dependencies in the trust base would be a
sandbox in name only, and nothing generated needs it yet.

**Done when:** an application can be built from its own source, run, stopped,
archived, restored and deleted through the CLI. The build path exists and is
tested; what it has no source to build is supplied by Phase 2, so this closes
there rather than here.

## What comes next

### Phase 2 — Generation

Done so far:

| | |
|---|---|
| Content-addressed versions, and the permission delta between two ([ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md)) | ✅ |
| `AgentProvider`, with model output as validated proposals rather than commands | ✅ |
| The deterministic mock provider, and a CI job that keeps generation offline | ✅ |
| The bounded plan → generate → build → test → repair loop | ✅ |
| `ephemeral generate` — an application reaching `Ready` without anybody writing code | ✅ |

Still to do:

| | |
|---|---|
| A real provider, behind the same trait | |
| Regenerating an existing application, with the permission delta put to the user | |
| Rolling back to a version that worked | |

**Done when:** the CSV comparator can be built from a natural-language request
end to end, with CI exercising the whole journey against the mock provider and
never calling a real model.

### Phase 3 — Permissions

Enforcement, not just modelling: meta-permissions wired to real operations, app
permissions enforced at the runtime boundary, the permission UI, the audit log
in the loop, and the sandbox.

**Done when:** every test in `tests/security.rs` is backed by an enforcement
point rather than by the domain model alone.

### Phase 4 — Desktop

The Tauri shell, the dashboard, state visualisation, logs, permission
management.

**Done when:** somebody who has never used a terminal can ask for an app, decide
its permissions, use it, and delete it.

### Phase 5 — Cross-platform

Windows, macOS and Linux to parity, then the mobile control plane and clients
([ADR-0007](architecture/decisions/0007-mobile-control-plane.md)).

### Phase 6 — Hardening

The [threat model](../SECURITY.md#threat-model), security testing, supply-chain
work, performance, recovery, installers and release automation.

**Done when:** the threat model is written and every mitigation it names either
exists or is recorded as accepted risk.

### Phase 7 — Sharing

Giving an application to somebody else, publishing it, and — separately — letting
several people use one running instance. Designed in [sharing.md](sharing.md);
decided in [ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md),
[ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md) and
[ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md).

Sits on top of nearly everything else: it needs a runtime to build a received
recipe, generation to produce versions, sandboxing to make accepting a
stranger's application reasonable, and the threat model — shared sessions are
the largest expansion of it so far.

One part is **not** deferrable to Phase 7: immutable, content-addressed versions
belong with Phase 2, because that is when versions start being produced and
identity cannot be retrofitted onto history that was never recorded.

**Done when:** an application can be published to a git host, installed by
somebody else, and run under permissions *they* granted — with an update that
wants more than the version they approved refused until they decide.

**Decided:** a group operates its own relay, and there is no other kind —
Ephemeral does not run one and there is no third-party opt-in. That costs reach
deliberately: a group where nobody will keep a device on does not get a shared
session. [ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md)
is accepted.

## Things deliberately not being built yet

Not everything absent is an oversight. These are decided-against-for-now, with
the reasoning recorded:

- **App-to-app composition.** Reserved in [ARCHITECTURE.md](../ARCHITECTURE.md#12-app-to-app-composition)
  as explicit capability contracts. Not in the MVP.
- **Plugins.** The seams exist; the plugin system does not.
- **Cloud sync.** Desktop is local-first, and stays useful without a server.
- **A WebAssembly runtime.** The most likely future addition to the runtime
  trait, and the reason the trait exists — but today's ecosystem cannot run the
  general case.
- **A central Ephemeral registry.** Git hosting already distributes recipes, and
  a curated registry implies a safety judgement the project is not in a position
  to make ([ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md)).
