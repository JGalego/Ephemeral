# Development

How to work on Ephemeral. For how to *use* it, see [install.md](install.md).

## Prerequisites

Deliberately short. Bootstrap installs what it can and tells you about the rest.

| | |
|---|---|
| **Rust** | Installed via [rustup](https://rustup.rs). The exact toolchain is pinned in `rust-toolchain.toml` and rustup will fetch it for you. |
| **Git** | Any recent version. |
| **Docker** | **Optional.** Nothing in the development workflow needs it. Ephemeral detects it at runtime and explains its absence ([ADR-0005](architecture/decisions/0005-docker-first-runtime-abstraction.md)). |
| **Python + Pillow + numpy** | **Optional**, and only to regenerate `docs/assets/banner.gif`. The output is committed, so you never need these to build or run anything. |

## Bootstrap

```bash
git clone https://github.com/JGalego/Ephemeral.git
cd Ephemeral

./scripts/bootstrap        # macOS / Linux
scripts\bootstrap.ps1      # Windows
```

It installs the pinned toolchain and its components, reports whether Docker is
available without treating its absence as a failure, then builds and tests
everything. Safe to run repeatedly.

## The loop

```bash
cargo test --workspace                 # everything
cargo test -p ephemeral-core           # just the domain
cargo test --test security             # just the security invariants
cargo run -p ephemeral-cli -- doctor   # check your environment
cargo run -p ephemeral-cli -- states   # the whole lifecycle machine
```

Before pushing:

```bash
./scripts/check
```

That runs exactly what CI runs, in the same order, so a local pass means a
remote pass. It is worth the thirty seconds.

## Trying the CLI without touching your real data

`EPHEMERAL_HOME` points Ephemeral somewhere else entirely:

```bash
export EPHEMERAL_HOME=/tmp/ephemeral-scratch

cargo run -p ephemeral-cli -- create "compare two CSV files"
cargo run -p ephemeral-cli -- generate <app-id>   # needs Docker; uses the mock provider
cargo run -p ephemeral-cli -- review <app-id>     # decide what it may do
cargo run -p ephemeral-cli -- run <app-id>
cargo run -p ephemeral-cli -- audit
```

Everything except `generate` and `run` works without Docker. `ephemeral doctor`
says what this machine is missing and what would fix it.

Delete the directory when you are done. Nothing outside it was touched.

## How the code is arranged

```text
crates/
  ephemeral-core/    the domain: manifests, lifecycle, permissions, audit,
                     retention, storage. No Docker, no network, no platform
                     APIs. This is where the security-critical logic lives.
  ephemeral-runtime/ the sandbox. What actually confines generated code, and
                     the Docker implementation of it.
  ephemeral-agent/   the boundary with whatever writes the code. Model output
                     is a validated proposal, never a command. The mock
                     provider here is what CI runs against.
  ephemeral-cli/     a client of the above. Decides nothing itself.
docs/
  architecture/decisions/   ADRs — read 0001 first
scripts/
  bootstrap, check          development entry points
```

Two boundaries hold the design together, and both are enforced rather than
merely intended:

**The core performs no host I/O.** With the `fs` feature off it touches nothing
outside the process, and CI builds it that way. If you find yourself reaching for
`std::fs` or a network call in `ephemeral-core`, the thing you are writing
belongs in another crate behind a trait.

**Clients decide nothing.** The CLI does not evaluate a permission, compute a
transition, or join a path. A client that reimplemented any of that would be a
second, subtly different Ephemeral.

## Testing

Four layers. The third is not optional.

**Unit tests** live next to the code and must not need Docker, the network or a
model. They are the bulk of the suite.

**Integration tests** cover core ↔ runtime, core ↔ platform adapter, core ↔ a
generated app. Mostly Phase 1 onwards.

**Security tests** live in `crates/ephemeral-core/tests/security.rs` and run as
their own CI job so a failure is legible at a glance. Every one is an executable
statement of something [SECURITY.md](../SECURITY.md) promises:

- an app inherits nothing from Ephemeral
- app A cannot reach app B's grants or storage
- a denied permission is actually denied
- the agent cannot grant, approve, or destroy
- a deleted app loses every permission at once
- secrets cannot reach a manifest or the audit record

If one fails, that is a vulnerability, not a broken test. If you add a security
boundary, add the test that proves it holds.

**End-to-end tests** cover the whole journey against the deterministic mock
provider. **CI never makes a live model call** ([ADR-0008](architecture/decisions/0008-agent-provider-abstraction.md));
a test that needs one will not be merged.

## Style

`cargo fmt` decides formatting; clippy runs with `-D warnings`. If a lint is
genuinely wrong, `#[allow]` it locally with a comment saying why — there are a
few of those in the tree and each explains itself.

Beyond that:

- Public items in `ephemeral-core` carry doc comments. It is the domain model;
  it should read like documentation.
- Comments explain *why*, not *what*. The code already says what.
- Prefer explicit over clever in security code. A permission check should be
  readable by someone who does not write Rust.
- No `unwrap()`/`expect()` outside tests and startup.

## The supply chain

`deny.toml` allows permissive licences only, fails on known vulnerabilities, and
refuses any dependency from a source we did not name. `cargo deny check` runs in
CI and in `./scripts/check`.

It will occasionally refuse something you want. That is the policy working: the
first dependency to hit it was the obvious crate for locating a platform data
directory, which pulls in a weak-copyleft transitive dependency. We wrote the
thirty lines instead. Prefer that outcome to an exception, and if an exception is
genuinely right, record why.

## Adding a dependency

Ephemeral runs untrusted code, so a dependency is a security decision, not a
convenience one. Before adding one, ask whether the code you would write instead
is smaller than the code you are pulling in — surprisingly often it is.

If you do add one: pin it, commit the lockfile, and check `cargo deny` passes.

## Commits and ADRs

[Conventional Commits](https://www.conventionalcommits.org/), small and
coherent, each leaving the repository building. If your change makes a decision
somebody would otherwise have to reverse-engineer — a framework, a protocol, a
security boundary, a persisted format — write an ADR. Copy
[the template](architecture/decisions/0000-template.md) and take the next
number.

An ADR that lists no rejected alternatives is not an ADR.

Full detail in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Regenerating the banner

```bash
python3 scripts/render-banner.py
```

Needs Pillow and numpy, which are deliberately not project dependencies. The
output is committed and rendering is deterministic, so re-running it without
changing anything reproduces the same file rather than a spurious diff.
