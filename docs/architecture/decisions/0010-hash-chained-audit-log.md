# ADR-0010: An append-only, hash-chained audit log with redaction on write

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation

## Context

Ephemeral makes security-relevant decisions continuously and largely
autonomously: granting a permission, mounting a directory, exposing a port,
creating a container, reading a secret, deleting an app. After the fact, two
different people need answers from the record. A user asks *"what did this thing
do with my files?"* An incident responder asks *"was this permission ever
granted, by whom, and what ran afterwards?"*

Both are worthless if the record can be quietly edited — including by generated
code that found a way to write to it — or if it is so noisy that the important
entries are invisible.

There is also a failure mode specific to this product: a log is exactly where
secrets leak. Environment values, tokens and credentials get written into
diagnostics by well-meaning code, and once written they persist in a file that
is, by design, never modified.

## Decision

**An append-only audit log, hash-chained for tamper evidence, with redaction on
the write path.**

- **Append-only.** No update, no delete. Retention policy may age out old
  entries wholesale, but an individual entry is never rewritten.
- **Hash-chained.** Each entry carries the hash of its predecessor and a hash of
  its own content, so any modification, reordering or excision within the chain
  is detectable by verification. Verification is exposed via `ephemeral doctor`.
- **Distinct from observability.** Logs, build output and lifecycle history
  serve *understanding*; the audit log serves *security*. They have different
  audiences, retention and integrity requirements, and merging them would drown
  the security signal.
- **Redaction happens on write, not on display.** Registered secret values and
  known credential patterns are scrubbed before an entry is constructed. A
  display-time filter is not a control — it fails the moment anything else reads
  the file.
- **Secret *access* is logged; secret *values* never are.** "App X read secret
  `API_KEY`" is exactly the right entry.
- **Every entry names the actor** — user, Ephemeral, agent, runtime, system —
  and every security-relevant decision carries its reason.
- **Every entry is machine-readable and human-readable.** A user sees
  *"You denied network access to Apartment Comparator"*; a responder gets
  structured fields.

## Alternatives considered

### A plain application log with a `security` level

Nearly free, and reuses existing tooling. Rejected because there is no integrity
property at all, security entries are buried in operational noise, and normal
log rotation destroys exactly what an investigation needs.

### Cryptographically signed entries with a per-install key

Stronger than hashing: an attacker without the key cannot forge a valid chain,
whereas a hash chain can be wholly recomputed by anyone who can rewrite the
file. Rejected for Phase 0 because the key must live somewhere the attacker
cannot reach, and on a compromised host — the case that matters — that is not
achievable locally. It becomes worthwhile once a remote verifier exists, and is
the natural upgrade when the control plane lands (ADR-0007).

### Append to the OS audit subsystem (auditd, Windows Event Log, ASL)

Real OS-level integrity, tooling and centralisation. Rejected as the primary
store: it requires elevated privileges we do not want to demand, three
platform-specific implementations, and it cannot represent Ephemeral's own
domain concepts. A future *mirror* of high-severity events into the OS log is
worth doing.

### Full event sourcing — the audit log as the system's source of truth

Maximum fidelity, and no possibility of the log disagreeing with reality.
Rejected for the reasons in ADR-0004: deriving current state from a log makes a
truncated log produce wrong-but-plausible state, and the audit log's retention
needs differ from the domain's.

## Consequences

### What this makes easier

An honest answer to "what happened, and who decided it". Tamper *evidence*
without a key-management story. A place to point users that is genuinely
readable by them. A concrete, testable rule for contributors about where
security events go and what may never appear in them.

### What this makes harder

Every security-relevant code path must remember to write an entry, and a missing
entry is a silent gap — so audit coverage needs its own tests. Redaction must be
maintained as new secret shapes appear. Verification cost grows with chain
length, so the chain needs periodic checkpointing.

### What we are accepting

Tamper *evidence*, not tamper *resistance*: an attacker with write access to the
file and the ability to run our own hashing code can rebuild a consistent chain.
We state this plainly rather than implying stronger guarantees, and we plan the
upgrade path — signing plus off-host verification — rather than pretending it is
already here.

## Security implications

The audit log is a security control, so it is also a target. Consequently:
generated applications are never granted write access to it (nor to Ephemeral's
own installation); redaction is on the write path; the chain is verifiable on
demand; and a verification failure is surfaced to the user as a security event
rather than a warning in a file nobody reads.

## Revisit when

- The control plane exists and can act as an off-host verifier (→ add signing).
- Chain verification becomes slow enough to need checkpoints (→ add them, keep
  the property).
