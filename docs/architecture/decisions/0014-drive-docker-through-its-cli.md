# ADR-0014: Drive Docker through its CLI, not its HTTP API

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 1 — Local runtime

## Context

`DockerRuntime` has to pull images, create containers, apply resource limits,
mount approved directories, publish ports, stream logs, inspect health and tear
everything down. There are two ways to reach the daemon: the `docker` command, or
its HTTP API over the socket — in Rust, most likely via `bollard`.

Two things weigh unusually heavily here.

**This is the crate that confines untrusted code.** Its dependency tree is part
of the sandbox's trust base. `bollard` brings tokio, hyper, http, and their
transitive graph — a lot of code to audit, keep patched and satisfy the
supply-chain policy with, sitting directly in the security-critical path.

**Ephemeral has to explain itself.** Every privileged runtime operation is
audited, and users are told what was done on their behalf. A recorded API call
is an internal event; a recorded command line is something a person can read,
paste into a terminal, and verify.

## Decision

**Drive Docker by invoking the `docker` command**, with structured output
requested explicitly (`--format '{{json .}}'`) rather than parsed from human
formatting.

The container specification is built as an **argument vector by a pure
function**, separately from executing it. Nothing is passed through a shell.

## Alternatives considered

### The HTTP API via `bollard`

Typed requests and responses, no output parsing, no process-spawn overhead, and
proper streaming for logs and events. Genuinely the better engineering choice
for a system doing this at volume.

Rejected on two grounds. First, dependency weight in exactly the crate where it
costs most: an async runtime and an HTTP stack become part of what stands
between generated code and the host. Second, it talks to one socket, so it
misses the ecosystem the CLI gets for free — `DOCKER_HOST`, Docker contexts,
remote daemons, Docker Desktop's socket placement on macOS and Windows, and
Podman via its `docker` shim. Reimplementing that discovery correctly is more
work than the parsing it would save.

Ephemeral is not doing this at volume. It starts a handful of containers for one
person.

### Shelling out through `sh -c`

Trivially convenient for composing commands. Rejected outright: it puts a shell
between Ephemeral and the daemon, and the arguments include user-controlled
paths and model-generated values. There is no reason to introduce a quoting
problem that can be avoided by not having one.

### Both, behind a feature flag

Ship the CLI path, add the API path later for performance. Rejected for now as
two implementations of a security boundary — twice the surface to audit, and the
one nobody runs is the one that will be wrong.

## Consequences

### What this makes easier

A small dependency tree in the crate that most needs one. Working
out of the box with Docker Desktop, remote contexts and Podman. And an audit
trail made of commands a user can read and re-run themselves, which is
materially better than "created a container" for someone trying to understand
what happened to their machine.

Building the argument vector as a pure function means the **security-relevant
flags are unit-testable without Docker installed**: that `--cap-drop=ALL` is
present, that the network is denied unless granted, that ports bind to loopback,
that mounts are read-only unless write was granted. Those tests run everywhere,
including in CI, where no daemon exists.

### What this makes harder

Output parsing, which we constrain by always requesting JSON. Process-spawn
latency, irrelevant at this scale. Log streaming is a child process's stdout
rather than a socket, which is fine but less elegant. And CLI output formats can
change between Docker versions, so the parsing needs its own tests against
recorded fixtures.

### What we are accepting

A dependency on the `docker` binary being on `PATH`, which is the thing
`ephemeral doctor` already checks for and explains.

## Security implications

- The dependency tree of the confining crate stays small, which is the point.
- No shell, so no quoting or injection concerns on arguments that include
  user-supplied paths and model-generated values.
- Every invocation is recordable verbatim, so the audit log can say exactly what
  was run rather than paraphrasing it.
- Hardening flags become testable assertions in CI rather than claims in
  documentation.

## Revisit when

- Container volume grows enough that process-spawn cost or log-streaming
  fidelity actually matters.
- The CLI stops exposing something the API offers that a security control needs.
