# Architecture Decision Records

Decisions that shaped Ephemeral, why they were made, and what was rejected.

Read [ADR-0001](0001-record-architecture-decisions.md) first — it explains the
process. New records use [the template](0000-template.md) and take the next free
number.

ADRs are immutable once accepted. A decision that changes gets a new ADR that
supersedes the old one; the old one stays, marked superseded and linked forward.

| # | Decision | Status | Phase |
|---|----------|--------|-------|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | accepted | 0 |
| [0002](0002-rust-core-with-platform-shells.md) | A Rust core with thin platform shells | accepted | 0 |
| [0003](0003-two-tier-permission-model.md) | Two separate permission systems, with no inheritance | accepted | 0 / 3 |
| [0004](0004-explicit-lifecycle-state-machine.md) | Model the application lifecycle as an explicit event-driven state machine | accepted | 0 |
| [0005](0005-docker-first-runtime-abstraction.md) | A runtime abstraction, Docker-first on desktop but never required | accepted | 0 / 1 |
| [0006](0006-versioned-manifest-schema.md) | A versioned, self-describing application manifest | accepted | 0 |
| [0007](0007-mobile-control-plane.md) | Mobile executes through a control plane, and says so | accepted | 0 / 5 |
| [0008](0008-agent-provider-abstraction.md) | Provider-neutral generation, with a deterministic mock for CI | accepted | 0 / 2 |
| [0009](0009-storage-layout-and-retention.md) | Separated per-app storage, with retention as a first-class property | accepted | 0 / 1 |
| [0010](0010-hash-chained-audit-log.md) | An append-only, hash-chained audit log with redaction on write | accepted | 0 |
| [0011](0011-immutable-content-addressed-versions.md) | Application versions are immutable and content-addressed | accepted | 7 |
| [0012](0012-sharing-distributes-recipes.md) | Sharing distributes recipes, never grants | accepted | 7 |
| [0013](0013-how-several-people-share-an-application.md) | How several people share one application | **proposed** | 7 |
| [0014](0014-drive-docker-through-its-cli.md) | Drive Docker through its CLI, not its HTTP API | accepted | 1 |
| [0015](0015-defer-the-native-runtime.md) | Defer the native runtime rather than ship a weak one | accepted | 1 |
| [0016](0016-real-providers-live-in-their-own-crates.md) | Real providers live in their own crates,  and reach the network through `curl` | accepted | 2 |
| [0017](0017-mobile-generates-through-a-host-transport.md) | A phone generates for itself, through a C ABI and the host's own HTTPS | accepted | 5 |
| [0018](0018-android-is-an-application-and-jni-is-testable.md) | Android gets an application, and its JNI bridge is testable without a phone | accepted | 5 |
| [0019](0019-openai-compatible-and-a-local-model.md) | One OpenAI-compatible wire format, and a local provider that cannot leave the machine | accepted | 2 |
| [0020](0020-the-host-chooses-the-provider.md) | The host chooses the provider, on every platform | accepted | 5 |
| [0021](0021-webassembly-is-the-runtime-a-phone-can-have.md) | WebAssembly is the runtime a phone can have | accepted | 1 |
| [0022](0022-how-an-interpreter-reaches-a-device.md) | How an interpreter reaches a device | accepted | 1 |
| [0023](0023-a-confined-application-reaches-the-network-through-its-host.md) | A confined application reaches the network through its host | accepted | 1 |

## When you need an ADR

Write one if your change:

- establishes or moves a security boundary
- selects a major framework, protocol or dependency
- defines a format that gets persisted or exported
- would otherwise have to be reverse-engineered from the code by the next
  contributor

Routine implementation choices do not need one. If you cannot name an
alternative you rejected, you probably do not need an ADR.
