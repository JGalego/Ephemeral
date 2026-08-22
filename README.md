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
[![Status](https://img.shields.io/badge/status-phase%204%20·%20desktop-a3e635.svg)](docs/roadmap.md)
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

**Phases 0–4 are done.** An application can be described, written, built, tested
and run without anybody touching its code; what it may do is enforced rather
than described; and all of it works from a window as well as a terminal.

Generation was not called finished until somebody watched it happen: a sentence
into a real `docker build`, an image out, the application running in the sandbox
and printing the right answer — with the mock provider and with a real model,
which wrote a working application and passed its own tests first time. What was
[actually run](docs/roadmap.md#not-a-claim-this-time-it-was-run) is written down,
including the two bugs it found.

Permissions are now consulted by the things that act. A capability needs both
halves — the application allowed to have it, and Ephemeral allowed to carry it
out — so one revocation of Ephemeral's authority empties every sandbox at once,
and anything running on what was just taken back is stopped rather than keeping
it. [Every promise is mapped to the code that enforces
it](docs/security/enforcement.md), which is a claim you can check rather than
take.

The window is not a second Ephemeral. Generating and running live in one crate
both clients call, so an application started from a window is confined exactly
as one started from a terminal, and both report it in the same words.

Ephemeral is built in phases, and each phase must demonstrably work before the
next one starts. See [the roadmap](docs/roadmap.md) for detail, and
[docs/sandbox.md](docs/sandbox.md) for exactly what the sandbox holds.

| Phase | Scope | Status |
|-------|-------|--------|
| 0 | Repo, docs, architecture, CI, app model, state machine | ✅ done |
| 1 | Docker runtime, sandbox, run/stop/watch, logs, cleanup | ✅ done |
| 2 | Provider abstraction, generation agent, build/test/repair loop | ✅ done — and [watched happen](docs/roadmap.md#not-a-claim-this-time-it-was-run), against real Docker and a real model |
| 3 | Meta-permissions, app permissions, permission UI, audit, sandboxing | ✅ done — [every promise mapped to what enforces it](docs/security/enforcement.md) |
| 4 | Desktop application and dashboard | ✅ done — generate, run, pause, roll back, decide, and Ephemeral's own authority, without a terminal |
| 5 | Windows, macOS, Linux, then mobile | 🚧 the desktop three and Android ship installable builds; iOS ships the engine, not yet an app |
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
                                    # --provider mock (no credential), local (nothing
                                    # leaves the machine), anthropic or openai
$ ephemeral review <app>            # decide what it may do, one question at a time
$ ephemeral run <app> -- /data/left.csv /data/right.csv
$ ephemeral rollback <app> <digest> # back to a version that worked
$ ephemeral publish <app> ./my-app  # an ordinary directory; git init and push
```

No Docker daemon? `EPHEMERAL_CONTAINER_COMMAND=podman` runs the whole thing
without one.

Not willing to send what you asked for to a company? `--provider local`
generates against a model server on this machine — Ollama by default, or
llama.cpp, LM Studio or vLLM — and refuses any endpoint that is not a loopback
address. It is the only real answer to "my intent leaves the machine"; a model
small enough to run on a laptop is also likelier to return something Ephemeral
refuses to act on, and that trade is yours to make. `--provider mock` produces a
genuine working CSV comparator with no model at all.

**What you cannot do yet.**

- **Trust the desktop window on your platform.** Everything the terminal does
  can be done in it now, its rendering is tested in a headless browser, and the
  real window has been run and filmed under WebKitGTK on a virtual display
  ([`tests/film-window.sh`](apps/desktop/tests/film-window.sh)) — which is how
  the last bug in it was found. Nobody has yet opened it on macOS or Windows,
  and those use a different WebView again.
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

Or install a build rather than making one:
[the releases page](https://github.com/JGalego/Ephemeral/releases) carries
`.deb`, `.rpm` and AppImage for Linux, a universal `.dmg` for macOS, both
installers for Windows, and an `.apk` for Android, alongside CLI archives for
six targets. None of them is signed by a known identity, so every platform will
say so in its own way — macOS and Windows warn, and Android refuses to upgrade
one release over another. [docs/install.md](docs/install.md) says exactly what
you will see and what to do about it.

The Android app records and generates; it does not build or run what it
generated, because a phone has no sandbox to run it in. That limit is on its
first screen, and the reasoning is in [docs/mobile.md](docs/mobile.md).

See [docs/development.md](docs/development.md) for the full development guide.

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
