# The sandbox

What actually happens when a generated application runs, and what stops it doing
anything else.

Ephemeral's permission model decides what an application *may* do. The sandbox is
where that decision becomes a fact about a running process. This page is the
list of what the sandbox holds, and — as importantly — what it does not.

## The one rule everything else follows from

**An application's confinement is built from what its owner granted, never from
what its manifest asks for.**

A manifest is written by a model. If the sandbox were built from it, an
application could widen its own confinement by asking for more. So the sandbox
is built from the ledger — the record of decisions a *person* made — and a
manifest that requests the whole filesystem gets exactly as much access as one
that requests nothing, until somebody says otherwise.

## What holds, whatever was granted

These are not derived from any permission, because none of them is negotiable.

| Control | What it does |
|---|---|
| `--cap-drop=ALL` | Every Linux capability dropped. Nothing generated needs to bind a low port, change ownership or load a module. |
| `--security-opt=no-new-privileges` | No path from inside the container to more privilege than it started with, which is what makes dropping capabilities durable rather than advisory. |
| `--read-only` | The image's filesystem cannot be modified. The only writable places are the two below. |
| `--tmpfs /tmp` (`noexec`, `nosuid`, `nodev`, 64 MiB) | Scratch space that cannot be used to drop something and run it. |
| `--user` (never root) | Runs as your own identity where the platform has one, `nobody` otherwise. There is no configuration that produces uid 0. |
| `--restart no` | Ephemeral's state machine decides when something runs. A container that resurrects itself would be a state machine that is not in charge. |

## What each grant actually buys

| You granted | The sandbox gets |
|---|---|
| nothing | `--network none`, no mounts, no ports. It can see nothing of yours. |
| `read:~/Downloads/**` | That directory bind-mounted at `/mnt/downloads`, **`readonly`**. |
| `write:~/Reports/**` | That directory bind-mounted, writable. |
| `listen:8080` | The port published on **`127.0.0.1`** only, on a Docker network created with `--internal` — reachable from your machine, unable to reach off it. Listening is not calling out. |
| `outbound:*` | Ordinary networking. This is the only way an application reaches the internet. |
| `outbound:api.example.com` | **Nothing — the application refuses to start.** See below. |

Every application also gets its own storage at `/data`, which holds only what it
put there.

## Two things it will not do

**It will never mount a whole root.** `~/**`, `/**` and `C:/**` are refused even
when granted, and the refusal is reported rather than silently dropped. That
promise is in [SECURITY.md](../SECURITY.md), and it does not depend on anybody
reading a prompt carefully.

**It will never substitute a weaker control for one it cannot apply.** Docker
has no per-destination egress filter. An application whose owner allowed four
hostnames cannot be given ordinary networking, because that is the whole
internet rather than four hostnames — so Ephemeral refuses to start it and says
what it would take:

```console
$ ephemeral run price-watcher
error: cannot enforce network access limited to api.example.com: Docker cannot
filter outbound traffic by destination, and running this application with
ordinary networking would give it the whole internet instead of the sites you
allowed. Honouring this needs a filtering proxy, which Ephemeral does not have
yet.
```

That refusal is a feature. The alternative is an application running with more
access than its owner believes it has.

## Secrets

A secret value never enters a command line. The sandbox specification carries
the *names* of settings an application was allowed to read; the values are
supplied separately and passed through the `docker` process's own environment,
so they are absent from the process table, from error messages, and from the
audit log.

That is what makes the audit record worth reading: Ephemeral can write the
command it ran verbatim, and you can paste it into a terminal yourself.

## The one claim you should check yourself

Every other claim on this page is a unit test. This one is not: whether Docker
will publish a port on an `--internal` network cannot be established without a
daemon, and there is none in CI. If it turns out not to work, an application
that listens **refuses to start** rather than quietly receiving ordinary
networking — the failure mode is safe — but the feature would be broken and
worth knowing about.

Ten seconds on a machine with Docker settles it:

```console
$ docker network create --internal ephemeral-isolated
$ docker run --rm --network ephemeral-isolated -p 127.0.0.1:8080:80 \
    alpine sh -c 'echo it published'
```

If that prints `it published`, the combination works. If Docker refuses it,
Ephemeral says so in those terms rather than relaying the raw error.

## How this is tested

The confinement is decided by pure functions — a specification in, an argument
vector out — so every claim on this page is a unit test that runs on a machine
with no container runtime at all. Building `ephemeral-runtime` without its
`daemon` feature removes everything that can spawn a process and keeps
everything that decides anything, and CI builds it that way.

If you want to check a claim here, the assertions are in
`crates/ephemeral-runtime/src/docker/command.rs`. A change that weakens one
should be treated as a vulnerability rather than a refactor.

See [ADR-0005](architecture/decisions/0005-docker-first-runtime-abstraction.md)
for why containers, and
[ADR-0014](architecture/decisions/0014-drive-docker-through-its-cli.md) for why
the `docker` command rather than its API.

## When the record and reality disagree

A crash, a kill, or a purge while something was running can leave a container
behind. `ephemeral doctor` reports those, and `ephemeral cleanup` removes them —
after listing them, because a leftover container may still hold a mount of your
files and that is worth seeing before it goes.

Which containers count is decided by comparing what Docker holds against what
each application's *manifest* says it should hold. When the two disagree the
container is what is wrong, because the manifest is what you were shown. Only
containers carrying Ephemeral's own label are ever considered.

The other direction — an application whose record says it is running when its
container has crashed, exited or gone unhealthy — is `ephemeral status` for one
application, and `ephemeral watch` for all of them continuously.

`watch` is also the only thing that can enforce a wall-clock or disk limit,
because both are promises about what happens *over time*, and nothing else in a
one-shot CLI is around to watch it pass. Disk is measured over the application's
own data directory rather than asked of Docker, since that directory is a host
bind mount and is the thing that actually grows. It runs in the foreground and stops with Ctrl-C —
deliberately the least commitment available, since nothing about it decides
whether Ephemeral eventually has a background service, and a desktop shell can
host the same sweep unchanged.

## What is not here yet

- **An egress proxy**, which is what would make a hostname allow-list
  enforceable instead of a refusal.
- **A background supervisor.** `ephemeral watch` has to be running for crashes
  and time limits to be noticed as they happen. Nothing starts it for you yet.
- **`NativeRuntime`**, for what genuinely cannot be containerised. Deliberately
  not built ([ADR-0015](architecture/decisions/0015-defer-the-native-runtime.md)):
  the version reachable without new dependencies in the trust base would confine
  almost nothing, and an application declaring it is refused rather than run
  unconfined.

See [roadmap.md](roadmap.md).
