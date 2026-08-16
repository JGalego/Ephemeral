# Contributing to Ephemeral 🫧

Thanks for being here. Ephemeral is an open-source project and contributions are
welcome — code, documentation, security review, and hard questions about the
design all count.

## Before you start

- Read [ARCHITECTURE.md](ARCHITECTURE.md). Ephemeral has strong opinions about
  where code lives and what may talk to what; a patch that crosses those seams
  will be hard to merge no matter how good it is.
- Read [SECURITY.md](SECURITY.md). If you found a vulnerability, **do not open a
  pull request or an issue** — report it privately.
- Check [docs/roadmap.md](docs/roadmap.md). We build in phases and a phase is not
  advanced just because code exists.

## Getting set up

```bash
git clone https://github.com/JGalego/Ephemeral.git
cd Ephemeral
./scripts/bootstrap          # macOS / Linux
scripts\bootstrap.ps1        # Windows
```

The bootstrap script installs the toolchain components and dev tools the project
needs and verifies your environment. Full details in
[docs/development.md](docs/development.md).

Common tasks:

```bash
cargo test --workspace          # tests
cargo fmt --all                 # format
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo run -p ephemeral-cli -- doctor                    # diagnostics
```

`./scripts/check` runs everything CI runs, in the same order. Run it before
pushing and you will rarely be surprised.

## Making a change

1. **Branch** from `main`.
2. **Keep commits small and coherent.** Every commit should leave the repository
   building and passing tests. A commit that does one thing is easier to review,
   revert and bisect than a commit that does nine.
3. **Write the tests with the feature**, not afterwards. See the testing section
   below for what "with" means for security-relevant code.
4. **Update the documentation** that your change makes untrue.
5. **Open a pull request** describing what changed, why, and what you considered
   and rejected.

### Commit messages

[Conventional Commits](https://www.conventionalcommits.org/). The type and scope
are used to generate the changelog, so they matter.

```text
feat(core): add application manifest
feat(runtime): add docker runtime
feat(security): add application permission model
fix(runtime): clean up orphaned containers
test(e2e): add csv comparator lifecycle
docs: document runtime architecture
ci: add cross-platform build matrix
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`,
`chore`, `revert`. Scopes in use: `core`, `cli`, `runtime`, `agent`, `platform`,
`security`, `ui`, `docs`, `ci`.

Please do not write `implement ephemeral`.

### Authorship

Commits are authored by the person making them. Do not add `Co-Authored-By:`
trailers naming a tool or an assistant, and do not let one set itself as the
author — see [CLAUDE.md](CLAUDE.md).

### Architecture Decision Records

If your change makes a decision that a future contributor would otherwise have
to reverse-engineer — a framework, a protocol, a security boundary, a storage
format — write an ADR in `docs/architecture/decisions/`. Copy
[the template](docs/architecture/decisions/0000-template.md), take the next
number, and describe the alternatives you rejected and why. An ADR that lists no
rejected alternatives is not an ADR.

Superseded ADRs are not deleted; they are marked superseded and linked to their
replacement.

## Testing

Four layers, and the third one is not optional for security-relevant code.

**Unit tests** — the state machine, permissions, manifests, storage, retention.
These live next to the code and must not need Docker, the network, or an LLM.

**Integration tests** — core ↔ runtime, core ↔ platform adapter, core ↔ a
generated app.

**Security tests** — a first-class layer, in `tests/security/`. Every one of
these is an executable statement of an invariant we promise:

- app A cannot read app B's files
- an app cannot reach Ephemeral's secrets
- a denied permission is actually denied at the enforcement point
- a container cannot escape its intended mounts
- resource limits are applied
- a deleted app loses runtime access

If you add a security boundary, add the test that proves it holds. If you find a
way past one, that is a vulnerability report, not a PR.

**End-to-end tests** — at least one complete journey: prompt → plan → generate →
build → test → run → inspect → stop → archive → restore → delete, against the
deterministic mock provider.

**CI never depends on a live LLM call.** If your test needs a model, it needs
the mock provider. Non-deterministic tests will be reverted.

## Definition of done

A change is done when:

- the code exists and handles its errors
- tests exist at the right layers
- the documentation that your change affects is updated
- the security implications were considered and stated in the PR
- CI passes
- the logs it produces are useful to somebody debugging at 2am
- the UX is understandable by someone who is not a developer
- platform-specific behaviour is accounted for, or explicitly deferred with a
  note
- it introduces no undocumented privilege escalation

That last one is the one we will hold up a PR over.

## Code style

- `cargo fmt` decides formatting. Do not argue with it in review.
- `clippy` runs with `-D warnings`. If a lint is genuinely wrong, `#[allow]` it
  *locally* with a comment saying why.
- Public items in `ephemeral-core` carry doc comments. It is the domain model;
  it should read like documentation.
- Prefer explicit types over clever ones in security code. The permission check
  should be readable by someone who does not write Rust.
- No `unwrap()`/`expect()` outside tests and program startup.

## Code of conduct

Be decent to each other. Assume good faith, critique the design rather than the
person, and remember that the person on the other end is doing this voluntarily.
Behaviour that makes the project worse to participate in will be moderated by
the maintainers.

## Licence

By contributing you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
