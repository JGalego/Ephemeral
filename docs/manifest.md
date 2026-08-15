# The application manifest

The durable description of one generated application. Everything else about an
application is disposable by design — the source can be regenerated, the
container rebuilt, the logs discarded. This is the one artifact that has to
survive an upgrade, an export, or a restore from an archive made months ago.

It is also a **security document**: it is what you read to decide what an
application may do. A change to its meaning changes what a past approval meant,
which is why the schema is versioned and why unknown versions are refused rather
than guessed at.

Rationale and rejected alternatives:
[ADR-0006](architecture/decisions/0006-versioned-manifest-schema.md).

## An example

```yaml
schema_version: 1
id: apartment-comparator-3f2a1b9c
name: Apartment Comparator
description: Compares two CSV files of apartment listings and shows the differences.
version: 1
created_at: 2026-08-15T13:00:00Z
updated_at: 2026-08-15T13:04:00Z

lifecycle:
  state: ready
  repair_attempts: 1
  repair_budget: 3

runtime:
  type: docker
  image: python:3.12-slim@sha256:...
  interface: web
  port: 8080
  entrypoint: [python, serve.py]

permissions:
  filesystem:
    - read: ~/Downloads/apartments/**
  network:
    outbound: false
  process:
    execute: false
  devices:
    camera: false
    microphone: false
    location: false

resources:
  cpu_millis: 500
  memory_mib: 512
  storage_mib: 1024
  max_processes: 64
  max_runtime: 15m

budget:
  max_repairs: 3
  max_duration: 30m
  max_spend_cents: 500

artifacts:
  source: source
  build: build
  logs: logs

metadata:
  purpose: Compare the two listing exports I downloaded.
  retention:
    policy: temporary
    retain_for: 1w
  execution:
    where: local
```

YAML for people, JSON for machines — the same model, so nothing can mean one
thing in one form and something else in the other.

## The fields

### Identity

| Field | Notes |
|-------|-------|
| `schema_version` | **Required.** Checked before anything else is interpreted. |
| `id` | Stable for the application's whole life. Validated: `[a-z0-9-]`, ≤ 64 chars. |
| `name` | What you see. |
| `description` | What it does, in your terms. |
| `version` | Incremented when the application is regenerated or repaired. |

### `runtime` — optional until planning settles it

`None` until planning has decided what kind of program this needs to be. An
application that has only been *requested* genuinely does not know yet, and
recording a placeholder would be a guess presented as a fact.

`validate()` requires it from the first build onwards — precisely the states in
which something acts on it (`building`, `validating`, `repairing`, `ready`,
`starting`, `running`, `paused`, `stopping`, `unhealthy`).

`type` is `docker`, `native` or `remote`. `interface` is `web`, `command_line`,
`api`, `worker` or `job`, and decides what "open it" means. `entrypoint` is a
list, already split into arguments — there is no shell in the path, so there is
no shell injection.

Images should be pinned by digest. An image reference that can change underneath
a "reproducible" application is not reproducible.

### `permissions`

Empty means nothing, and that is the default. See
[permissions.md](permissions.md).

Note what this block records for environment settings: **names only**. The
values live in platform-native secure storage and are injected by the runtime.
There is no field to put a secret in, which is a stronger guarantee than a rule
saying not to.

### `resources` and `budget`

Every limit is a real ceiling, and a zero is *invalid* rather than meaning
unlimited — that is how ceilings get removed by accident. A manifest that means
unlimited says so by omitting the field.

`budget.max_repairs` seeds the lifecycle's repair budget, so the limit the
manifest declares is the limit the state machine enforces. Two numbers that could
disagree would mean the visible limit is not the real one.

### `artifacts`

Paths **relative to the application's own directory**, validated on every load.
Absolute paths, drive letters, UNC prefixes and `..` segments are all refused —
a manifest is a document a user can edit and an attacker might supply, and these
paths get joined onto a real directory.

### `metadata`

`purpose` is the intent in the user's own words. It is the durable object in
Ephemeral: the application satisfying it can be regenerated, repaired or thrown
away, and the intent survives all of it.

`retention` is how ephemeral the application is; `execution` says whether it runs
on this device or on a control plane, and names which one. That is surfaced
rather than hidden, because if an application runs on a server then your data
goes to a server, and that is the most important thing to know before handing
over a file.

## Versioning rules

**Reject rather than guess.** A manifest with an absent or unknown
`schema_version` is refused whole. Guessing the version of a security document
means guessing what a user consented to.

**Additive changes within a version.** New optional fields with safe defaults may
be added to version *N*. Anything that removes a field, changes its meaning, or
**broadens what a permission expression allows** requires a new version.

**Explicit, tested migrations.** Upgrading is a named function with round-trip
tests and fixtures for every historical version, kept forever.

**Deny-biased defaults.** A field absent from an older manifest defaults to the
*least* privilege. A permission that did not exist in version 1 is denied for a
version 1 manifest, never assumed.

**Portable.** No absolute host paths, no machine identifiers, no secret values.
A manifest describes an application, not the machine it happens to be on.

## What is refused

All of these are errors, not warnings:

| | Why |
|---|---|
| An unknown or missing `schema_version` | Guessing means guessing what was consented to |
| An unrecognised key | A typo that silently disables a restriction leaves you believing you restricted something |
| An id like `../../etc` | It is used as a path component |
| A scope like `~/Downloads/../../etc/shadow` | Deserialisation runs the same parser as construction |
| An artifact path outside the app's directory | It is joined onto a real directory |
| A zero resource limit | Silently meaning "unlimited" is how ceilings vanish |
| A containerised runtime with no image | It would fail later, less legibly |
| A web interface with no port | It could never be opened |
| No runtime, once the app can build | Everything downstream would act on a guess |

## Reading one

```console
$ ephemeral inspect apartment-comparator-3f2a1b9c
```

The file itself is at
`<data-root>/apps/<app-id>/manifest.json` and is meant to be readable. It is
written atomically through a temporary file in the same directory, so a crash
mid-write leaves the previous manifest intact rather than a half-written
document describing permissions nobody approved.
