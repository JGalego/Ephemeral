# The sandbox

What actually happens when a generated application runs, and what stops it doing
anything else.

Ephemeral's permission model decides what an application *may* do. The sandbox is
where that decision becomes a fact about a running process. This page is the
list of what the sandbox holds, and — as importantly — what it does not.

There are two of them. Most of this page is about **the container**, which is
what a desktop with Docker uses and what the tables below describe. The second
is **WebAssembly**, which is what runs where there is no daemon — a phone above
all — and which holds by a different mechanism. It has its own section at the
end, and the one rule below governs both.

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

## Verified against a real runtime

Everything above has been checked against a running container, not only against
the arguments Ephemeral produces. Asking Podman what it actually applied:

```text
ReadonlyRootfs: true          NetworkMode:  none
CapDrop:        every one     SecurityOpt:  no-new-privileges
Memory:         536870912     MemorySwap:   536870912   (equal — no swap)
PidsLimit:      64            RestartPolicy: no
User:           65534:65534
```

**No daemon required.** Podman is a drop-in replacement that runs containers
without a background service, which matters on a machine where one cannot be
started. Point Ephemeral at it with `EPHEMERAL_CONTAINER_COMMAND=podman`; nothing
else changes, because Ephemeral asks for nothing Podman does not implement.

The one thing still unverified is whether a port can be published on an
`--internal` network — the reference application is a command-line tool and does
not listen. If it turns out not to work, an application that listens **refuses
to start** rather than quietly receiving ordinary networking, and Ephemeral says
so in those terms.

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

## The other sandbox: WebAssembly

Where there is no container runtime, an application runs as a WebAssembly
module inside Ephemeral itself
([ADR-0021](architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md)).
This is not the weaker option with a label on it. It is a different shape of
argument.

A process starts with the whole machine, and confining it means taking things
away — a list of removals that is only as good as its completeness. A
WebAssembly module starts with **nothing**. It has no syscalls. It cannot name
a file, open a socket, read the clock or learn its own process id unless the
host hands it a function that does so. Forgetting to add something yields
*less* access rather than more.

| Control | What it does |
|---|---|
| Preopened directories | The only directories that exist. A file descriptor can only be derived from one already held, so a module given none can open nothing at all. |
| No sockets | Not blocked — absent. There is no networking in this WASI implementation to import, so a module asking for one fails to start rather than failing at its first request. |
| Fuel | Executed instructions, not seconds. It cannot be escaped by sleeping, blocking or being descheduled, and it is the same bound on a fast phone and a slow one. |
| Memory ceiling | Applied per linear memory by the store. Growth past it traps rather than returning a failure the application might ignore. |
| Unresolved imports | A module that imports anything the host did not provide **never instantiates**. This is not a check Ephemeral performs; there is nothing for the import to bind to. |

Two consequences worth stating plainly:

- **It is stricter than the container about the network.** An egress grant
  cannot be honoured here at all, so an application that was granted one is
  refused with an explanation rather than started without it.
- **A user interface costs no permission.** An application whose interface is
  `web` writes a page and the host renders it — there is no port, no server and
  no socket. The usual arrangement needs a network permission, and that same
  permission then lets the application talk to anybody.

What it does not have is anything to generate for it yet: an application must
be a compiled `.wasm` module, or a script whose interpreter is installed. See
[roadmap.md](roadmap.md).

## What is not here yet

- **An egress proxy**, which is what would make a hostname allow-list
  enforceable instead of a refusal.
- **A background supervisor.** `ephemeral watch` has to be running for crashes
  and time limits to be noticed as they happen. Nothing starts it for you yet.
- **`NativeRuntime`**, for what genuinely cannot be containerised. Deliberately
  not built ([ADR-0015](architecture/decisions/0015-defer-the-native-runtime.md)):
  the version reachable without new dependencies in the trust base would confine
  almost nothing, and an application declaring it is refused rather than run
  unconfined. WebAssembly covers most of what it was wanted for, and confines.
- **An interpreter for the WebAssembly runtime.** Without one it runs compiled
  modules only, and nothing generates those yet.

See [roadmap.md](roadmap.md).
