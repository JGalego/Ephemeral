# ADR-0013: A shared instance has a host, and the host is a trust boundary

- **Status:** **proposed** — the execution-location question is open and needs a
  product decision. See [Open question](#open-question).
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 7 — Sharing

## Context

[ADR-0012](0012-sharing-distributes-recipes.md) covers giving somebody an
application: they rebuild it, with their own data and their own permission
decisions. That model is safe precisely because nothing is shared at runtime.

It does not cover the thing people most want to demonstrate:

> "Build me a group chat, and let my friends join with a link."

Here there is **one** application, **one** body of state, and **several
people**. That is a different problem in three ways the current design does not
handle:

**There is more than one user.** `Actor::User` is a singleton today — "the
person at this machine", unnamed. The moment somebody joins by invite, the
person using the application is not the person who granted its permissions.

**The application processes other people's data.** Everything in the permission
model reasons about protecting *the user* from *the application*. A shared
instance also has to reason about protecting participants from each other, and
all of them from whoever runs it.

**It has to be somewhere.** A local-first application that several people use at
once is a contradiction unless something is reachable by all of them. Desktop
Ephemeral has no such thing today, and ports bind to loopback by design.

The temptation is to treat this as a networking feature. It is not. It is a
change to who the product is protecting, from whom.

## Decision

**A shared instance is a service with a host, and Ephemeral says so.**

Whoever runs the instance can see everything in it. That is not a flaw to be
engineered around; it is the truth about hosting, and the design's job is to
make it legible rather than to obscure it — the same way execution location is
surfaced rather than hidden ([ADR-0007](0007-mobile-control-plane.md)).

Concretely:

### Identity

Each Ephemeral installation holds a keypair, generated on first use and kept in
platform-native secure storage. A participant *is* a public key, with an
optional display name that is a label rather than an identity. No accounts, no
directory, nothing central.

### An invite is a capability, not a link to a door

An invite is a signed, scoped token naming exactly what it confers, and it is
presented in the same terms as a permission:

> **Ana is inviting you to Group Chat, running on Ana's computer.**
>
> You will be able to: read and send messages in this room.
> You will not be able to: change the app, see other rooms, or read anything
> else on Ana's computer.
>
> Messages you send are stored on Ana's computer. Ana can read them.
>
> This invite expires in 7 days and Ana can revoke it at any time.

Invites are scoped, expiring by default, individually revocable, and recorded in
the audit log on both sides. Accepting one is a decision with the same weight as
granting a permission, because that is what it is.

### Three planes of permission, kept apart

Collapsing any two of these is how this goes wrong:

| Plane | Governs | Decided by |
|-------|---------|------------|
| Host's app permissions | What the application may touch on the **host's** device | The host |
| Guest's app permissions | What the application may touch on the **guest's** device — their camera, a file they pick | The guest, on their own machine |
| Participation capability | What a guest may do **inside** the application — send messages, invite others, moderate | The host, via the invite |

The third is new. Application-level authorisation is the application's own
concern, but Ephemeral supplies the primitive so that generated code does not
reinvent it — badly — every time. A generated application must not be the thing
deciding who is allowed to use it.

### Hosting is an explicit, revocable state

An application is not shareable by default. Hosting is a distinct action with
its own consent, its own audit trail, and its own off switch that ends every
session immediately. Exposing a port beyond loopback is part of that decision
and is never implied by anything else.

## Open question

**Where does a shared instance actually run?** This determines the privacy
properties, the operating cost, and how much infrastructure the project takes
on. It is a product decision rather than a technical one, so it is recorded here
unresolved.

### Option A — On the host's own device

The person who created the application runs it; guests connect to them.

- **For:** no infrastructure to operate; local-first is preserved; data stays on
  one identifiable person's machine; the trust boundary matches an intuition
  people already have from hosting a game server. Costs nothing to run.
- **Against:** the host must be online for anyone to use it; NAT traversal needs
  at least a rendezvous service, so "no infrastructure" is not quite true; the
  host's address is exposed to guests; effectively unavailable as a *host* on
  mobile.

### Option B — On an Ephemeral control plane

The instance runs on servers, as mobile execution already will.

- **For:** always available; no NAT problem; works when the author's laptop is
  shut; the mobile design needs a control plane regardless
  ([ADR-0007](0007-mobile-control-plane.md)), so this reuses it.
- **Against:** everyone's data leaves every device; the project takes on
  hosting, cost, abuse and moderation; it creates a high-value target holding
  many users' data and running untrusted generated code multi-tenantly;
  local-first stops being true for the feature people will demo most.

### Option C — Peer-to-peer with replicated state

No host at all; participants exchange updates directly.

- **For:** no server, no host who can read everything, nothing to operate.
- **Against:** substantially the hardest; generated applications would have to
  be written against a conflict-free replication model, which is a strong
  constraint on what a model can produce; presence and discovery still need a
  rendezvous; "who is allowed to join" without a host is an unsolved-enough
  problem that it would dominate the schedule.

### Recommendation

**A, with B available as an opt-in**, and C left open.

A keeps the product honest — local-first, no infrastructure, a trust boundary
that maps onto something people already understand. B exists anyway for mobile,
so offering it as a deliberate choice ("run this where it is always available,
knowing the data goes there") costs little beyond what is already planned, and
the interface already has to distinguish local from remote execution.

Starting with B would be easier and would quietly make Ephemeral a cloud
product with a local mode, which is the opposite of the stated direction.

## Consequences

### What this makes easier

The demo everybody wants — an application built from a sentence, shared by link,
used by several people. A vocabulary for multi-user applications that generated
code can rely on instead of improvising. A migration path towards
capability-based delegation, which
[ADR-0003](0003-two-tier-permission-model.md) deliberately left room for.

### What this makes harder

Nearly everything. Identity, key management, transport security, NAT traversal,
revocation that actually takes effect on a running session, and — the deepest
one — a permission model that currently assumes a single user learning to
express several. `Actor::User` becomes an identity rather than a singleton,
which touches the lifecycle, the ledger and the audit log.

### What we are accepting

That a shared instance is not local-first for its guests, whichever option wins,
and that the host can read everything. We accept both and disclose them
prominently rather than designing an illusion of privacy we cannot deliver.

## Security implications

This is the largest expansion of the threat model since the product began, and
it must not be built before
[the threat model](../../../SECURITY.md#threat-model) exists. New concerns, none
of which the current design addresses:

- A **malicious host** — sees all participant data by construction; the design
  must state this rather than mitigate it.
- A **malicious guest** — attacking the application, the host's machine, or
  other participants through the application.
- **Invite leakage** — an invite is a capability, so treating it as a secret,
  scoping it, expiring it and revoking it are all load-bearing.
- **Prompt injection through other people's content** — a message written by a
  guest is untrusted input that may later reach a generation or repair run. This
  is the existing prompt-injection problem with a new, cheap delivery route.
- **Exposure beyond loopback** — the single largest change to the desktop
  attack surface so far, and the reason hosting is its own explicit decision.
- **Revocation must be real** — ending a session must terminate access
  immediately, not merely stop issuing new invites.

## Revisit when

The open question is answered. Until then this ADR is a description of the
problem and not a licence to build.
