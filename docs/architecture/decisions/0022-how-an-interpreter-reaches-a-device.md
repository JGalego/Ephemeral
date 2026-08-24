# ADR-0022: How an interpreter reaches a device

- **Status:** **proposed.** The mechanism is the owner's to choose — it is a
  supply-chain decision, and this record exists to make choosing easy rather
  than to make the choice.
- **Date:** 2026-08-24
- **Deciders:** Ephemeral maintainers
- **Phase:** 1 — Local runtime

## Context

[ADR-0021](0021-webassembly-is-the-runtime-a-phone-can-have.md) gave Ephemeral a
runtime that works everywhere, including on a handset with no daemon. It is
built, and every client drives it: `ephemeral run` in a terminal, the Run button
in the window, `ephemeral_run` on a phone. All three reach the same function.

One link is open, and it is the one that decides whether any of it is useful.

**Nothing generates WebAssembly.** `Program::locate` resolves an application to
something runnable in two ways:

| The manifest says | What runs | What is needed |
|---|---|---|
| `program.wasm` | the application itself | a toolchain, somewhere |
| `main.js`, `main.py`, … | an interpreter, given the script | that interpreter, as a module |

The first is the fast path and a desktop could produce it. It is not the path
that matters, because the thing this was all for is a phone, and nothing on a
handset can compile anything. The second is the only reading under which
somebody describes an application on a train and runs it before they get off.

`interpreters/javascript.wasm` and `interpreters/python.wasm` do not exist, and
are not committed here. A megabyte of unreviewable binary in a repository is the
one thing this project has been most careful not to accumulate, in the one place
where what the bytes do is the entire question — and an interpreter is the most
privileged thing that would ever be added, because every application runs
*inside* it.

Teaching the planner to target this runtime before the interpreter exists would
be worse than doing nothing. Applications would generate cleanly and then fail
with "the JavaScript interpreter is not installed", which says less than
today's honest "this needs a computer with Docker".

## What any candidate has to be

Decided already, by the code, and worth writing down because it narrows the
field before anybody argues about vendors:

- **A WASI preview 1 command module.** It has `_start`, it reads `args_get`, and
  it opens files through preopened directories. `Program::Interpreted` hands it
  the script as `/program/<name>` and nothing else.
- **Able to run a script it is pointed at.** Not one embedded into it at build
  time. Ephemeral generates the script seconds before running it, so an
  interpreter that must be recompiled per application is a toolchain by another
  name, and the toolchain is the thing a phone does not have.
- **Small enough to sit in a phone's storage**, and honest about it in the
  interface. A JavaScript engine is on the order of a megabyte; a full Python is
  more than an order of magnitude larger. That difference is a product decision,
  not only a technical one.
- **Reviewable in the sense that matters here**: a build that can be reproduced
  from published source, pinned by digest, by somebody who wants to check.

No specific build has been evaluated. Doing that evaluation *is* the decision,
and it belongs to whoever will be answerable for what ships.

## The three ways it could arrive

### Fetched on first use, pinned by digest

Ephemeral downloads it the first time an application needs it, verifies it
against a digest committed here, and refuses to run anything if the digest does
not match.

**For:** nothing large in the repository; the digest is the reviewable artefact
and it is one line; a new interpreter is a version bump rather than a release.
**Against:** the first run of the first application needs a network, which is a
poor moment to discover that; Ephemeral would grow a downloader, which is a
thing that fetches and executes code and therefore wants the same scrutiny as
the sandbox; and it puts a hard dependency on somebody else's hosting.

### Built in CI and attached to a release

A workflow builds it from pinned source and publishes it with the release, so
installing Ephemeral installs an interpreter.

**For:** what ships is built from source this project pinned, by a pipeline
anybody can read; no runtime download; the provenance question has a good
answer for once.
**Against:** the release grows by the size of every interpreter offered; CI has
to carry a C toolchain and a wasi-sdk; and a build that is not reproducible
means the digest proves only that we all got the same bytes, not what they are.

### Shipped inside the Android package

The `.wasm` is an asset in the APK, unpacked to `interpreters/` on first run.

**For:** it is there before the application is; nothing to fetch; the store
already distributes and verifies the package.
**Against:** every user downloads every interpreter whether they use one or not;
the APK grows by all of them; and it answers the question for one platform,
leaving the desktop needing one of the other two anyway.

These are not exclusive. The likely shape is the second for the desktop and the
third for Android, which is one build and two ways of delivering it — but that
is a guess, and the point of this record is that somebody should decide rather
than discover.

## Consequences of leaving it open

**Ephemeral generates for containers and runs WebAssembly, and those are
different sets.** That is the state today, it is stated in the roadmap and in
`docs/mobile.md`, and every refusal names which of the two an application has
hit. Nothing pretends otherwise, which is the only reason leaving it open is
tolerable.

**The runtime is exercised, not theoretical.** Every test that runs an
application under it assembles a module from WebAssembly text, so the sandbox,
the bounds, the argument vector, the page rendering and the audit record are all
checked against real execution. What is untested is the one thing that needs a
real interpreter: whether a program a model wrote in a language somebody uses
actually works.

**Whoever decides this inherits a small surface.** `Program::locate` already
names the file it wants and where it wants it, `INTERPRETERS` is one table, and
the error a person sees is already written. Adding an interpreter is putting a
file in a directory; the decision is only about which file, and how it got
there.

## Alternatives considered

### Commit a `.wasm` and move on

Rejected, and it is worth saying why sharply: it would be the largest
unreviewable artefact in the repository, it would be the most privileged code in
the product, and nobody reviewing a pull request would look at it. Every other
binary decision here has gone the other way for smaller stakes.

### Write the interpreter

A small language implemented in Rust and compiled to WebAssembly, or interpreted
directly. Rejected: it makes Ephemeral responsible for a language, and a
generated application would be written in a dialect a model has never seen —
which is the opposite of the reason models are useful here.

### Run the model's source through a host-side interpreter instead

Skip WebAssembly for scripts and execute them in an interpreter linked into
Ephemeral. Rejected in ADR-0021 and rejected again here: the sandbox would then
be that interpreter's own, which for CPython is nothing at all, and it picks the
language for every application forever.

### Require every application to be compiled

Only accept `.wasm`, and let a desktop produce it. Rejected: it is the state of
the world today, and the phone — which is what ADR-0021 was for — can never
participate. It is a decision to build the runtime and not use it.
