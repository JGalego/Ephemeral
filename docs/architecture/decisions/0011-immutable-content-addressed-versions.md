# ADR-0011: Application versions are immutable and content-addressed

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 7 — Sharing (model), with groundwork in Phase 2

## Context

Today an application carries `version: u32`, a counter bumped when it is
regenerated or repaired. That is enough to say "this changed" and nothing else.
It cannot answer any of the questions that actually come up:

- The repair loop produced build #17, #18 and #19. #18 worked and #19 does not.
  How do I go back?
- I want to give this application to somebody. What exactly am I giving them,
  and how do they know they got the same thing?
- I used this last month and it has been regenerated since. What changed?
- **Version 2 wants network access and version 1 did not. Who told me?**

That last question is the one that matters. An application's version is not
just a revision of its code — it is a revision of *what it is permitted to do*.
Every app store in existence has got this wrong at some point by letting an
update quietly carry a wider permission set than the version the user approved.
Ephemeral has a permission model good enough to get it right, and would throw
that away by treating versions as an incrementing integer.

Sharing makes it urgent. Two people cannot talk about "the CSV comparator"
unless there is something precise that names *which* CSV comparator.

## Decision

**A version is an immutable, content-addressed snapshot of everything that
determines what an application is and what it may do.**

- Every generation, every successful repair and every manifest change that
  alters behaviour produces a **new version**. Versions are never edited in
  place.
- A version is identified by a **digest** over its recipe: the manifest (minus
  instance-local fields such as lifecycle state and timestamps), the generated
  source, the runtime specification including pinned image digests, and the
  declared permissions. Same digest, same application, on any machine.
- `version: u32` stays as the human-facing sequence number. The digest is the
  identity.
- The chain of versions is kept, so a user can see what changed and go back to
  one that worked. Rolling back is *selecting an existing version*, never
  mutating a current one.
- **The permission delta between two versions is a first-class, surfaced fact.**
  Moving an application to a version that requests anything the running version
  did not is a permission decision, presented with the same
  [`PermissionPrompt`](../../permissions.md#asking) machinery as any other, and
  refused by default until a person decides. Narrowing needs no prompt.
- Instance identity (`AppId`) and version identity (the digest) are separate.
  The id says *which installation*; the digest says *which application*.

## Alternatives considered

### Keep the integer counter

Nothing to build. Rejected because it cannot express rollback, cannot identify
an application across machines, and — decisively — gives the permission-delta
problem nowhere to live. An update would silently carry whatever permissions the
new manifest happened to declare.

### Put every application in a real git repository

Full history, diffs, branches, merges, and tooling everybody already knows. Very
tempting, and it is close to what `ephemeral publish` produces for the *export*
format ([ADR-0012](0012-sharing-distributes-recipes.md)). Rejected as the
*internal* model because git's identity is a commit over a working tree, which
includes plenty that is not part of the application's identity, and because
mutable branches and rewritable history are the opposite of what a permission
record needs. An application's version must be a fact, not a pointer somebody
can move.

### Semantic versioning declared by the generation agent

Human-meaningful, and familiar. Rejected as the identity because the agent is
not a trusted actor ([ADR-0008](0008-agent-provider-abstraction.md)) and a
version number it chose is an assertion, not a measurement. Two applications
claiming `1.2.0` need not be the same application. A digest cannot be wrong in
that way. Semantic versions may still be *displayed* for applications published
deliberately; they are never the thing identity is checked against.

### Snapshot the built artifact rather than the recipe

Address the built image, so the recipient runs bit-for-bit what the author ran.
Stronger reproducibility. Rejected because it means shipping executables between
users, which makes the recipient's safety depend on trusting the sender — the
one thing the sharing model exists to avoid
([ADR-0012](0012-sharing-distributes-recipes.md)). Addressing the recipe keeps
the recipient's own Ephemeral in the loop as the thing that builds and confines.

## Consequences

### What this makes easier

Rollback becomes selection rather than regeneration. "What changed?" is
answerable, including for permissions. Two people can name the same application
precisely. Publishing has something well-defined to publish. The repair loop
gains a natural artifact per attempt, which makes *build #17 → failure → repair
→ build #18 → passed* a real record rather than a log line.

### What this makes harder

Versions accumulate and need a retention policy of their own — the product would
look silly filling a disk with history of applications designed to be
disposable. Digest computation has to be stable across platforms and over time,
which means being explicit about canonical serialisation and about which
manifest fields are part of identity.

### What we are accepting

Storage growth, bounded by a retention policy on version history. And the
discipline that anything affecting identity must be canonicalised carefully — a
digest that changes because a map serialised in a different order is worse than
no digest at all.

## Security implications

Strongly positive, and the main reason for the decision.

- **An update cannot widen what an application may do without a person
  deciding.** The permission delta is computed from the manifests rather than
  claimed by whoever produced the new version.
- A digest lets a recipient verify they got what was offered, which is the
  precondition for accepting an application from somebody they do not trust.
- Immutable versions mean the audit trail can name exactly which application was
  running when something happened, rather than "the CSV comparator, some
  version".
- Because identity covers the *recipe* and not the built artifact, a compromised
  builder cannot substitute a different application without changing the digest.

## Revisit when

- Version history growth becomes a real user complaint (→ tighten retention).
- Reproducible builds become strong enough that addressing the artifact adds
  something the recipe digest does not.
