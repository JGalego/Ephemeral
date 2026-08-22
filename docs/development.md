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

## The desktop window

```bash
cd apps/desktop/src-tauri && cargo run          # needs a display
cd apps/desktop/src-tauri && cargo test
cd apps/desktop && npm install playwright && node tests/render.test.mjs
```

It is **its own workspace**, excluded from the root one. Tauri brings a large
native dependency tree and system libraries that only exist on a machine with a
desktop, and everything else here stays buildable and testable without them —
`cargo test --workspace` never touches it.

On Linux it needs `libwebkit2gtk-4.1-dev librsvg2-dev patchelf`. The frontend
has no build step and no framework: a window that shows a list does not need a
supply chain.

Everything that decides *what* to show is a pure function in `ui/render.js`,
tested in headless Chromium. A desktop UI that can only be exercised by opening
it is a UI nobody tests, and what this one renders is the permission prompt.

### Looking at it without a display

A UI no human has seen has problems no test finds. Both of these produce files
rather than opening a window, so looking is possible on a machine with no
display — CI runners, containers, a session over ssh:

```bash
cd apps/desktop
node tests/film.mjs          # the frontend, driven through a real interaction
tests/film-window.sh 20      # the real Tauri window, under Xvfb
```

`film.mjs` drives the actual `ui/` modules in Chromium against view data shaped
exactly as `ephemeral-api` serialises it, and writes `recordings/` — a webm plus
a still per step, each named for what you are meant to check in it. It needs
Playwright. `film-window.sh` runs the built binary against a virtual X server
and records the framebuffer with ffmpeg; it needs `xvfb` and `ffmpeg`, which is
why neither is in `./scripts/check` or CI.

Neither asserts anything. That is the point — **you have to look at the
frames.** Everything below was found that way, with the whole suite passing:

- a granted permission still offering "Allow", in the same colour as an
  unanswered one
- a critical permission with a `type allow` field and nothing to submit it
- answering a request throwing you back to the list, so the page confirming
  what you had allowed was never seen
- a refusal rendered below the fold, hundreds of pixels from where the person
  was looking
- an app allowed to reach the whole internet drawn on the list exactly like one
  that can see nothing of yours
- granting something recolouring it green and fading it, so the most dangerous
  grant on the page became the calmest thing on screen
- the window telling you it "is not running inside Ephemeral" — while running
  inside Ephemeral, because `withGlobalTauri` was unset and nothing had ever
  connected the tested rendering to the tested commands
- the list saying "Running" about a container that had exited long before, found
  by `film-window.sh` and by nothing else: the rendering was right and the record
  was stale, which is a class of bug the Chromium film cannot see because it has
  no machine underneath it

Each has a test now, and every one of those tests was written after looking.
When you fix something a film found, add the assertion and say in the comment
that a film is what found it — the next person will otherwise assume the tests
were sufficient, which is exactly the assumption that let these through.

`recordings/` is gitignored. A committed film ages into a picture of a bug
somebody already fixed, and it looks exactly as current as the code beside it.

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
