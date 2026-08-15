<div align="center">

<img src="docs/assets/logo.svg" alt="" width="136" height="136">

# Ephemeral 🫧

### Software that exists only while it's useful.

Describe what you need in your own words. Ephemeral builds a small app that does
exactly that, runs it in a sandbox, shows it to you — and throws it away when
you're done.

**Software, on demand.**

[![CI](https://github.com/JGalego/Ephemeral/actions/workflows/ci.yml/badge.svg)](https://github.com/JGalego/Ephemeral/actions/workflows/ci.yml)
[![Licence](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-dea584.svg?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Status](https://img.shields.io/badge/status-phase%202%20·%20generation-f0abfc.svg)](docs/roadmap.md)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20·%20Windows%20·%20Linux%20·%20iOS%20·%20Android-8ab4f8.svg)](ARCHITECTURE.md)

</div>

## What is this?

Today, if you need a tool, you go looking for an app. You install it, it asks for
permissions you don't understand, and it stays on your machine forever — even
though you needed it for ten minutes.

Ephemeral flips that around. You say what you want to do:

> "Compare these two CSV files and show me what's different."

Ephemeral builds a small application that does exactly that, runs it in a safe
box, shows it to you, and throws it away when you're done.

You never have to know how it was built.

## How it works, in plain language

1. **You ask.** In your own words.
2. **Ephemeral plans.** It decides what kind of program is needed and what it
   will need access to.
3. **Ephemeral builds it.** It writes the program and sets up everything the
   program needs to run.
4. **Ephemeral tests it.** If it doesn't work, Ephemeral fixes it and tries
   again — up to a limit you control.
5. **Ephemeral asks permission.** Before the program touches your files, your
   network, or your camera, you get a plain-language question with a real
   answer to "why?".
6. **You use it.**
7. **It disappears.** Immediately, in a day, in a week, or never — your choice.

Every step is visible. When something is happening, Ephemeral tells you what and
why. No unexplained spinners.

## The two permission systems

This is the part that matters most, so it's worth saying twice.

**Ephemeral has permissions.** These control what the Ephemeral app itself may
do on your machine: run Docker, install runtimes, read directories, reach the
network.

**Every generated app has its own, separate permissions.** A generated app does
*not* inherit Ephemeral's permissions. Ephemeral may be allowed to read your
whole home directory; the CSV comparator it just built is allowed to read
exactly `~/Downloads/apartments` and nothing else.

Both sets are explicit, inspectable, and revocable at any time.

Generated code is treated as untrusted code. Always. An LLM wrote it — that is
not a reason to trust it.

## Status

**Phase 2 — Generation.** An application can now be described, written, built,
tested and run without anybody touching its code. The sandbox from Phase 1 is
what it runs in: a granted scope becomes a mount, an inbound port becomes a
loopback binding, and an application with an empty ledger gets a container that
can see nothing of yours.

Ephemeral is built in phases, and each phase must demonstrably work before the
next one starts. See [the roadmap](docs/roadmap.md) for detail, and
[docs/sandbox.md](docs/sandbox.md) for exactly what the sandbox holds.

| Phase | Scope | Status |
|-------|-------|--------|
| 0 | Repo, docs, architecture, CI, app model, state machine | ✅ done |
| 1 | Docker runtime, sandbox, run/stop/watch, logs, cleanup | ✅ done |
| 2 | Provider abstraction, generation agent, build/test/repair loop | 🚧 in progress |
| 3 | Meta-permissions, app permissions, permission UI, audit, sandboxing | 🚧 enforcement done, UI is the CLI |
| 4 | Desktop application and dashboard | ⏳ planned |
| 5 | Windows, macOS, Linux, then mobile | ⏳ planned |
| 6 | Threat model, security testing, supply chain, release automation | 🚧 [threat model](docs/security/threat-model.md) written |
| 7 | Sharing, publishing, and shared sessions | 🚧 publish/install done |

**What you can do today.** Describe what you want, have Ephemeral write and
build it, watch it fix its own build when it breaks, run the result in a
hardened container under exactly the permissions you granted, watch it for
crashes and time limits, read its output and the audit trail, and archive,
delete or purge it.

```console
$ ephemeral create "compare these two CSV files and show me what's different"
$ ephemeral generate <app>          # plan, write, build, test — bounded and cancellable
                                    # --provider mock (no credential) or anthropic
$ ephemeral review <app>            # decide what it may do, one question at a time
$ ephemeral run <app> -- /data/left.csv /data/right.csv
$ ephemeral publish <app> ./my-app  # an ordinary directory; git init and push
```

No Docker daemon? `EPHEMERAL_CONTAINER_COMMAND=podman` runs the whole thing
without one.

**What you cannot do yet.**

- **Run a model locally.** `--provider anthropic` works with an
  `ANTHROPIC_API_KEY`, and `--provider mock` produces a genuine working CSV
  comparator with no credential at all. What does not exist is an offline
  provider, which is the only real answer to "my intent leaves the machine".
- **Open a window.** There is no desktop application yet; the CLI is the whole
  interface, and both are clients of the same core.
- **Give an application a scoped list of hosts it may reach.** Docker cannot
  filter egress by destination, so an application granted one refuses to start
  rather than quietly receiving the whole internet. See
  [docs/sandbox.md](docs/sandbox.md).
- **Share a running session with other people.** Designed in
  [ADR-0013](docs/architecture/decisions/0013-how-several-people-share-an-application.md),
  not built.

The [threat model](docs/security/threat-model.md) lists what Ephemeral does
*not* defend against. It is the most useful page here for anyone deciding
whether to trust this.

## Try it (development)

Prerequisites and a one-command bootstrap:

```bash
./scripts/bootstrap        # macOS / Linux
scripts\bootstrap.ps1      # Windows
```

Then:

```bash
cargo run -p ephemeral-cli -- doctor    # check this machine
cargo run -p ephemeral-cli -- states    # the whole lifecycle state machine
```

See [docs/development.md](docs/development.md) for the full development guide
and [docs/install.md](docs/install.md) for end-user installation (once packages
are published).

## Documentation

| Document | What's in it |
|----------|--------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | How the system is put together |
| [SECURITY.md](SECURITY.md) | Security model and how to report a vulnerability |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to work on Ephemeral |
| [docs/concepts.md](docs/concepts.md) | The vocabulary: apps, intents, principals, retention |
| [docs/lifecycle.md](docs/lifecycle.md) | The application lifecycle state machine |
| [docs/permissions.md](docs/permissions.md) | Both permission systems, in detail |
| [docs/architecture/decisions/](docs/architecture/decisions/) | Architecture Decision Records |
| [docs/roadmap.md](docs/roadmap.md) | Phased plan |

## Design principles

1. Security before convenience.
2. Explicit permissions before implicit access.
3. Generated code is untrusted.
4. Every important operation is observable.
5. Everything is recoverable where practical.
6. The state machine is a first-class domain model.
7. The UI explains system behaviour rather than hiding it.
8. Provider and runtime independence.
9. Local-first on desktop.
10. No unnecessary cloud dependency.
11. No premature microservices.
12. Prefer boring infrastructure where it works.
13. Make the common path extremely simple.
14. Never sacrifice security for a flashy demo.
15. Build vertically and keep the system runnable.

## Licence

[Apache License 2.0](LICENSE).
