# ADR-0009: Separated per-app storage, with retention as a first-class property

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation (model), 1 — Local runtime (implementation)

## Context

A product called Ephemeral cannot treat deletion as cleanup code written at the
end. If apps accumulate, the product becomes the thing it was built to replace:
a machine full of software nobody chose to keep.

Storage also carries two security obligations. Apps must not be able to read
each other's data — the isolation promise depends on the layout, not only on the
sandbox. And a "deleted" app must actually lose access, including its runtime
resources, not merely disappear from a list.

Against that pulls data loss. An agent-driven system that deletes things is
frightening, and a user who loses work will not use it twice.

## Decision

**One predictable directory per app, with separated concerns, and a declared
retention policy on every app.**

```text
<data-root>/apps/<app-id>/
  source/      generated source
  build/       build output
  runtime/     runtime scratch — destroyed on teardown
  data/        the app's persistent data
  logs/        build, test and runtime logs
  artifacts/   exports and reports
```

- **The app id is the isolation unit.** Nothing outside `apps/<id>/` is reachable
  by that app, and no app's tree is mounted into another's runtime.
- **Secrets are never in this tree.** They live in platform-native secure
  storage; the tree holds references (ADR-0003, SECURITY.md).
- **Manifest and lifecycle history live in a record store** (SQLite) alongside
  the tree, so listing, searching and history do not require walking the
  filesystem, and a partially-deleted tree is detectable.

Retention is declared per app and changeable by the user at any time:

| Policy | Behaviour |
|--------|-----------|
| `one-shot` | created, run, deleted |
| `ephemeral` | expires quickly (default 24h) |
| `temporary` | remains dormant, expires (default 7d) |
| `reusable` | available until explicitly archived |
| `persistent` | behaves like a conventional application |

Deletion is **two-stage and recoverable by default**:

1. **Delete** — a tombstone. Runtime resources are destroyed immediately and
   irreversibly, the app loses all permissions and cannot execute, but its data
   remains restorable for a grace period.
2. **Purge** — irreversible removal of the tree and the record. Explicit, user-
   only, and audited.

The asymmetry is deliberate: **capability is revoked immediately, data is
retained briefly.** A deleted app must never be able to act, but the user must
be able to change their mind about the output.

## Alternatives considered

### One flat workspace shared by all apps

Simple and space-efficient. Rejected outright: it makes cross-app isolation
depend entirely on the runtime sandbox, with no structural backstop, and makes
per-app deletion an error-prone path-matching exercise.

### Everything in a database, including source and artifacts

Transactional deletion — a genuinely attractive property, since a partially-
deleted app becomes impossible. Rejected because generated source, build output
and logs must be inspectable, exportable and mountable into a container, and
forcing that through a database opposes the transparency the product promises.
We take the transactional half for records and keep bytes on the filesystem.

### Immediate hard deletion, no trash period

Truest to "ephemeral", and no lingering data to leak. Rejected because an
autonomous system that irreversibly deletes user data on its own schedule is
not trustworthy. The grace period is the compromise; a user who wants the pure
behaviour picks `one-shot` or purges explicitly.

### Content-addressed storage shared across apps, deduplicated

Space-efficient, and appealing when many apps share a base image or dependency
set. Rejected for user data: shared storage between isolated principals is an
isolation hazard, and reference counting introduces a class of bug where
deleting one app affects another. Image-layer sharing is left to the container
runtime, which already does it safely.

## Consequences

### What this makes easier

Per-app isolation with a structural backstop. Deletion, export and archive are
simple operations over a known tree. Retention sweeps are mechanical. A user can
open a directory and see exactly what an app is.

### What this makes harder

Some duplication across apps. Two systems of record — records in SQLite, bytes
on disk — that can disagree, requiring a consistency check (a `doctor` job).

### What we are accepting

Deleted data survives for a grace period, which is a small confidentiality cost
in exchange for recoverability. Users who want otherwise have `purge`, and the
grace period is configurable.

## Security implications

The layout is a load-bearing part of the isolation promise, and the security
test layer asserts it: app A cannot reach app B's tree; a deleted app loses
runtime access immediately; secrets never appear in the tree; runtime scratch is
destroyed on teardown. Purge must remove *everything* — tree, records, logs,
runtime resources and secret references — and that completeness is itself a
tested property.

## Revisit when

- Storage growth from duplication becomes a real user complaint.
- Retention defaults prove wrong in practice (they are guesses today).
