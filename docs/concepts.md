# Concepts

The vocabulary Ephemeral uses, and why each word means what it does. Every term
here is a type in `ephemeral-core`, so if the code and this page disagree, the
code is what runs and this page is the bug.

## Intent

**What the user actually wanted, in their own words.**

This is the durable object in Ephemeral. The application that satisfies it is
disposable implementation detail: it can be regenerated, repaired, rebuilt on a
different runtime, or thrown away and made again. The intent survives all of
that, which is why it is recorded on the manifest as `metadata.purpose` and why
it is what the interface shows you first.

> "Compare these two CSV files and show me what's different."

## Application

**One generated program, with an identity, a manifest and a life.**

Not an installed application in the conventional sense. It exists because you
asked for something, it does that thing, and then — depending on its
[retention policy](#retention-policy) — it goes away.

An application's identity is an `AppId`: lowercase, `[a-z0-9-]`, at most 64
characters, generated from its name with a random suffix. That constraint is not
cosmetic. The id is used as a filesystem path component and as the subject of
every permission grant, so an id containing `..` or a separator would let one
application's storage reach into another's. It is validated at construction, and
there is no way to build one that skips validation.

## Manifest

**The durable, portable description of an application.**

Identity, runtime, permissions, resource limits, artifacts, retention and
lifecycle state. Everything else about an application can be rebuilt; this is
the one artifact that has to survive an upgrade, an export, or a restore from an
archive made months ago.

It is also a security document: it is what you read to decide what an app may
do. Hence the versioned schema, the refusal to guess at an unknown version, and
the least-privilege default for every field. See [manifest.md](manifest.md).

## Principal

**Something that can hold a permission.**

```text
Principal::Ephemeral        the product itself
Principal::App(AppId)       one generated application
Principal::Plugin(PluginId) reserved for the future plugin system
```

Principals are isolated from each other, and **no principal inherits another's
grants**. Ephemeral being allowed to read your home directory grants a generated
app nothing at all. This is the central invariant of the permission model; see
[permissions.md](permissions.md).

## Actor

**Who caused something to happen.**

Distinct from a principal. A principal *holds* permissions; an actor *acts*.

```text
Actor::User        a human decision
Actor::Ephemeral   the product's own orchestration
Actor::Agent       the generation agent
Actor::Runtime     a container or process reporting a fact
Actor::System      the OS, a scheduler, a retention sweep
```

Actors are recorded on every lifecycle transition, every grant and every audit
entry, and certain operations are restricted to certain actors. `Actor::Agent`
cannot grant a permission, delete an application, or declare its own output
valid. That restriction is enforced in the core rather than requested in a
prompt, which is what makes it hold even when the agent has been successfully
steered by something it read.

## Permission

Two separate systems, which are never merged:

- A **meta-permission** is something *Ephemeral itself* may do — use Docker,
  install a runtime, execute a process, reach the network, read the keychain.
- An **application permission** is something *one generated app* may do, scoped
  as narrowly as practical.

Both are default-deny, revocable and auditable. Full detail in
[permissions.md](permissions.md).

### Scope

The narrowing on a permission. A `PathScope` is an anchored path, optionally
with `/**` meaning "and everything beneath":

| Written | Means |
|---------|-------|
| `~/Downloads/report.csv` | exactly that one path |
| `~/Downloads/apartments/**` | that directory and everything in it |

Containment is decided segment by segment, never by string prefix, so
`/home/user/docs` does not cover `/home/user/docs-private`. A `HostScope` does
the same for network destinations, respecting the dot boundary so
`*.example.com` covers `api.example.com` but not `example.com.attacker.net`.

## Lifecycle

**Where an application is in its life, and how it got there.**

An explicit, event-driven state machine: 20 states, 31 events, and a total
transition function. States never change by assignment — something *happens*,
and the machine decides what that means. See [lifecycle.md](lifecycle.md).

## Retention policy

**How ephemeral an application is.**

The property the product is named after, declared per application and changeable
at any time.

| Policy | Behaviour |
|--------|-----------|
| `one-shot` | created, run, deleted |
| `ephemeral` | expires quickly (24h by default) |
| `temporary` | stays dormant, then expires (7d by default) — the default |
| `reusable` | available until you archive it |
| `persistent` | behaves like a conventional application |

## Delete versus purge

Deliberately different operations, and the difference is the point.

**Delete** withdraws every permission immediately and stops the application
doing anything. Its record and its data stay, so you can change your mind. The
`Deleted` state *is* the tombstone.

**Purge** destroys the tree and the record. It is irreversible, user-only, and
audited.

The asymmetry is intentional: **capability is revoked at once, data is retained
briefly.** A deleted application must never be able to act, but you must be able
to change your mind about what it produced.

## Runtime

**What an application executes on**, behind a trait so more can be added:
`DockerRuntime` (the desktop default), `WasmRuntime` (an interpreter, and the
only one a phone can have — a module starts with no syscalls at all, so
confinement is its resting state rather than something applied to it),
`NativeRuntime` (modelled, deliberately unbuilt — see
[ADR-0015](architecture/decisions/0015-defer-the-native-runtime.md)), and
`RemoteRuntime` (modelled, unbuilt: a sandbox on a machine that has one, driven
from a device that does not).

The runtime is recorded in the manifest rather than decided at launch, so an
application's isolation is a durable fact rather than a property of whichever
machine happens to start it. It is `None` until planning settles it — an
application that has only been requested genuinely does not know yet what kind
of program it needs to be.

## Declared inputs

**What an application says it takes**, so something can ask for it. A name, a
label a person would recognise, a kind (text, number, file, folder, one of a
fixed set, or an on/off flag), whether it is passed positionally or under a
flag, and optionally a default and a line of help.

Every application generated so far has been a command-line tool with flags, and
a phone has no terminal to type one into. The alternative — asking a model to
write a user interface as well as a program — doubles what can go wrong for the
large majority of these things that are one input, one output and a couple of
options. So the application declares its shape and whatever is showing it draws
the form: one renderer per client rather than one per application.

**A declaration is not a permission.** An application saying it takes a file is
not an application that may read one; the person still chooses which, and the
sandbox still contains only what was granted. Declaring widens nothing, which is
what makes it safe for a model to write.

The argument vector is composed in the domain, never in a client. A phone, a
window and a terminal building commands separately are three subtly different
applications, and the one that gets a flag's default wrong sends a program the
opposite of what somebody chose.

## Workspace

**Everything Ephemeral keeps on one device**: the applications, the permission
ledger and the audit log, loaded together from one directory. You need the
manifest *and* the ledger to answer "may this app read that file?", and the
audit log as well to answer "what happened here?".

## Audit log

**The append-only, hash-chained record of security-sensitive operations.**

Separate from ordinary logging, which serves understanding rather than security.
Each entry carries the hash of its predecessor, so an edit, a reordering or a
removal fails verification. Redaction runs on the write path, so a secret cannot
enter the record even if a caller puts one in a reason string.

It offers tamper *evidence*, not tamper *resistance* — see
[ADR-0010](architecture/decisions/0010-hash-chained-audit-log.md), which is
candid about the difference.
