# ADR-0006: A versioned, self-describing application manifest

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation

## Context

The manifest is the durable description of a generated application: identity,
runtime, permissions, resource limits, artifacts, retention and lifecycle state.
Everything else about an app is disposable by design — the source can be
regenerated, the container rebuilt — so the manifest is the one artifact that
must survive upgrades, exports and restores from an archive made months ago.

It is also a **security document**. It is what the user reads to decide what an
app may do, and a change to its meaning changes what a past grant means. A
manifest whose interpretation silently shifts between versions is a privilege
escalation waiting to happen.

## Decision

Every manifest carries an explicit `schema_version` (currently `1`) as a
required field, and:

- **Reject rather than guess.** A manifest with an unknown or absent
  `schema_version` is refused with a typed error. There is no best-effort
  parsing of an unrecognised document.
- **Additive changes within a version.** New optional fields with safe defaults
  may be added to version *N*. Anything that removes a field, changes a field's
  meaning, or **broadens what a permission expression allows** requires a new
  version.
- **Explicit, tested migrations.** Upgrading a manifest is a named function with
  round-trip tests and fixture files for every historical version, kept in the
  repository forever.
- **Deny-biased defaults.** Any field absent from an older manifest defaults to
  the *least* privilege. A permission that did not exist in version 1 is denied
  for a version 1 manifest, never assumed.
- **YAML for humans, JSON for machines** — the same serde model. YAML because
  users read and edit manifests; JSON because it is what the API and storage
  layer exchange, with no ambiguity.
- **Portable by construction.** No absolute host paths, no machine-specific
  identifiers, no secret values — only references into secure storage.

## Alternatives considered

### No version field; infer the shape from the content

Less ceremony. Rejected immediately: inference on a security document means
guessing what a user consented to.

### A monotonically growing schema with only optional fields, never versioned

Works for a while and avoids migrations. Rejected because it cannot express a
*narrowing* or a re-interpretation — precisely the changes security work
produces — and it accumulates dead fields that no reader can safely ignore.

### Protobuf or another IDL-driven binary format

Real schema evolution rules, generated types, compact and fast. Rejected because
manifests are read and edited by users and diffed in review; a binary format
that requires tooling to inspect works against the transparency the permission
model depends on. Considered again for the wire protocol if the control plane
needs it — that is a separate decision from the on-disk manifest.

### JSON Schema as the normative definition, with types generated from it

Excellent for validation and for third-party tooling, and we may still publish
one. Rejected as the *source of truth* because Rust's type system already
expresses the constraints we care about more precisely — closed enums,
non-empty scopes, mutually exclusive modes — and two definitions of a security
document is one too many. A published JSON Schema is a derived artifact.

## Consequences

### What this makes easier

Archives restore correctly years later. Manifests move between machines and
platforms. Reviewers can diff exactly what an app is permitted to do. A schema
change is a visible, reviewable event rather than a silent drift.

### What this makes harder

Migration code and fixtures accumulate and must be maintained forever. Any
permission-broadening change forces a version bump, which is deliberate
friction on exactly the changes that deserve it.

### What we are accepting

Version 1 will be wrong about something. The migration machinery exists so that
being wrong is recoverable rather than permanent.

## Security implications

Deny-biased defaults mean an old manifest can never gain a capability by being
read by a newer Ephemeral. Rejecting unknown versions means a manifest crafted
for a different — perhaps future or forged — schema cannot be partially
interpreted into something permissive. Excluding secret values from the manifest
keeps the app description safe to display, export and share.

## Revisit when

Schema version 2 is needed — at which point this ADR gains a companion
documenting what changed and why.
