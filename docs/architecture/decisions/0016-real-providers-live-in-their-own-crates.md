# ADR-0016: Real providers live in their own crates, and reach the network through `curl`

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 2 — Generation

## Context

`ephemeral-agent` holds the `AgentProvider` trait and the deterministic mock.
CI asserts that no network client appears anywhere in that crate, which is what
makes "CI never makes a live model call" ([ADR-0008](0008-agent-provider-abstraction.md))
a fact rather than a policy nobody checks.

A real provider has to make an HTTPS request. That is in direct tension with the
guard, and resolving it badly would either weaken the guard or leave the product
permanently unable to talk to a model.

There is a second tension. The obvious way to make an HTTPS request in Rust is
`reqwest`, which brings tokio, hyper, rustls and a substantial transitive graph.
[ADR-0014](0014-drive-docker-through-its-cli.md) already rejected exactly that
trade for the Docker client, on the grounds that dependency weight matters most
in the crates closest to the security boundary. A provider is not the sandbox,
but it is the component that sends the user's own words off their machine.

## Decision

**Two things, both mirroring decisions already made here.**

**Real providers live in their own crates**, one per provider, named
`ephemeral-provider-*`. `ephemeral-agent` keeps the trait, the types and the
mock, and keeps its no-network guarantee — so the CI guard stays meaningful
instead of being relaxed to accommodate the first real implementation.

**They reach the network by invoking `curl`**, with the request body on stdin
and the credential passed through a `--config` document on stdin rather than as
an argument. The argument vector is therefore free of secrets and safe to write
verbatim into the audit log, exactly as the Docker argument vectors are.

The split that matters is inside the crate, not around it: **prompt
construction, request bodies and response parsing are pure functions**, tested
against recorded fixtures. The only untested-in-CI part is the process
invocation itself.

## Alternatives considered

### `reqwest`, or any Rust HTTP client

Typed, ergonomic, no subprocess, no dependency on something being installed.
Genuinely nicer to write against.

Rejected on the same two grounds as ADR-0014, plus one more. Dependency weight:
an async runtime and a TLS stack for a handful of requests per generation.
Environment: `curl` honours `HTTPS_PROXY`, corporate CA bundles, `.curlrc` and
client certificates without Ephemeral implementing any of it — and the machines
this runs on are more likely to be behind a proxy than not. And the third:
**the request becomes inspectable**. A user who wants to know what was sent to a
model provider can be shown a command, not told about a struct.

### Put the real provider in `ephemeral-agent` and relax the CI guard

One crate, no indirection. Rejected outright. The guard is the only mechanical
reason to believe CI is offline, and a guard that is relaxed the first time it
is inconvenient was decoration.

### Ship no real provider at all

The status quo, and defensible: the mock produces a genuinely working
application. Rejected because a product that cannot talk to a model is a
demonstration of a product, and the trait was designed to be implemented.

## Consequences

### What this makes easier

The offline guarantee stays mechanical. Adding a second provider is a new crate
rather than a change to a shared one, so providers cannot accumulate shared
state or drift into each other's behaviour. The dependency tree stays small
enough that `cargo deny` remains a useful signal rather than noise.

### What this makes harder

A dependency on `curl` being present — near-universal, and `ephemeral doctor`
checks for it and says so. Streaming responses are a subprocess's stdout rather
than a socket, which is fine and less elegant. And each new provider is a small
amount of transport code rather than a shared client.

### What we are accepting

**The transport is not exercised by CI.** Prompt building, request bodies,
response parsing and error mapping are all tested against fixtures; the actual
HTTPS call is not, and cannot be without a credential. This is a smaller
untested surface than any alternative that ships a real provider, and it is
stated rather than hidden.

## Security implications

- No secret ever appears in an argument vector, so a recorded command is safe to
  log and safe to show.
- A model's response is parsed as data and validated. A response that does not
  parse is an error, never a best-effort guess ([ADR-0008](0008-agent-provider-abstraction.md)).
- The credential is read from the environment or the platform keystore and lives
  in memory for the duration of one call.
- Sending an intent to a hosted provider means it leaves the machine. That is
  inherent, recorded in [the threat model](../../security/threat-model.md#t6--a-compromised-or-hostile-model-provider),
  and the answer is a local provider rather than a promise.

## Revisit when

- A provider needs something `curl` cannot express.
- Streaming becomes load-bearing for the interface rather than a nicety.
- A Rust HTTP client appears whose dependency tree is small enough to audit in
  an afternoon.
