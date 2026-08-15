# ADR-0013: How several people share one application

- **Status:** **proposed** — the recommendation changed once the question was
  put properly; see [Open question](#open-question). Needs a product decision
  before anything is built.
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

**Wherever a shared session lives, whoever holds it can read it — and Ephemeral
says so.**

That is the truth about shared state, and the design's job is to make it legible
rather than obscure it, the same way execution location is surfaced rather than
hidden ([ADR-0007](0007-mobile-control-plane.md)). What varies between the
options below is *who* that is: one participant, an operator, or nobody but the
participants themselves.

The rest of this decision holds whichever option wins.

### Identity

Each Ephemeral installation holds a keypair, generated on first use and kept in
platform-native secure storage. A participant *is* a public key, with an
optional display name that is a label rather than an identity. No accounts, no
directory, nothing central.

### An invite is a capability, not a link to a door

An invite is a signed, scoped token naming exactly what it confers, and it is
presented in the same terms as a permission:

> **Ana is inviting you to Group Chat.**
>
> You will be able to: read and send messages in this room.
> You will not be able to: change the app, or see other rooms.
>
> The app runs on your device, under permissions you grant. Messages are shared
> with everyone in the room.
>
> This invite expires in 7 days and Ana can revoke it at any time.

The specific disclosure depends on where the session lives, and must name it
exactly — "stored on Ana's computer, where Ana can read them" is a different
sentence from "shared directly between participants", and a user deciding
whether to join needs the true one.

Invites are scoped, expiring by default, individually revocable, and recorded in
the audit log on both sides. Accepting one is a decision with the same weight as
granting a permission, because that is what it is.

### Three planes of permission, kept apart

Collapsing any two of these is how this goes wrong:

| Plane | Governs | Decided by |
|-------|---------|------------|
| Each participant's app permissions | What that participant's copy may touch on **their own** device — their camera, a file they pick | That participant, on their own machine |
| Session membership | Who is in the room at all | Whoever holds the session, via invites |
| Participation capability | What a member may do **inside** the application — send messages, invite others, moderate | The inviter, via the invite |

The third is new. Application-level authorisation is the application's own
concern, but Ephemeral supplies the primitive so that generated code does not
reinvent it — badly — every time. A generated application must not be the thing
deciding who is allowed to use it.

### Sharing is an explicit, revocable state

An application is not shareable by default. Starting a shared session is a
distinct action with its own consent, its own audit trail, and its own off
switch that ends every participant's access immediately — revocation has to take
effect on a live session, not merely stop new invites being issued.

If the chosen option requires accepting inbound connections, that is part of the
same decision and is never implied by anything else. It would be the single
largest change to the desktop attack surface so far, which is one of the
arguments against the options that need it.

## Open question

**Where does the shared part actually live?** This determines the privacy
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

### Option D — Distribute the app, relay only the session

Do not host the application at all. Every participant runs **their own copy**,
built from the shared recipe ([ADR-0012](0012-sharing-distributes-recipes.md)),
under permissions they granted. What is shared is the *session state* — the
conversation — carried by a relay.

- **For:** works on every platform, because each participant's app runs wherever
  their apps already run, including the control plane on mobile
  ([ADR-0007](0007-mobile-control-plane.md)) — no new hosting story. Survives
  anybody leaving: everyone owns the application, and the state is replicated
  rather than held by one device. A relay moves *data*, not code, so it is a far
  smaller trust and infrastructure surface than running untrusted generated code
  multi-tenantly — and it can be end-to-end encrypted, so the relay operator
  cannot read the contents. The relay is self-hostable, which makes "run your
  own" a real answer rather than a slogan.
- **Against:** generated applications must be written against a shared-state
  primitive rather than arbitrary networking. That is a genuine constraint on
  what a model may produce — though a much narrower one than Option C's "write
  your own conflict-free replication", because Ephemeral supplies the primitive
  and the application only reads and writes it. Some infrastructure still
  exists, and although contents can be encrypted, *metadata* — who is in a room,
  when they are active — is visible to whoever runs the relay. Conflict
  resolution has to be answered once, in the primitive, rather than avoided.

### Recommendation

**D.**

The first three options all took "share a running app" literally and went
looking for somewhere to run it. That framing conflates two separable things.
The **application** is already distributable as a recipe, and a recipe the
recipient rebuilt is *theirs* — it survives the author deleting theirs, and it
runs wherever that person's apps run. The only thing that genuinely needs a
shared home is the **session state**.

Separating them answers both of the questions that decide this:

| | A: host's device | B: control plane | C: peer-to-peer | D: distribute + relay |
|---|---|---|---|---|
| Works on every platform | Desktop hosts only — a phone can never host | Yes | Worst: *every* peer needs the runtime | Yes |
| Survives the author deleting it | No — it dies with their machine | Only with an ownership model | Yes | Yes |
| Who can read the contents | The host, entirely | Whoever operates it | Participants only | Participants only |
| Infrastructure to operate | A rendezvous service | Multi-tenant untrusted execution | A rendezvous service | A relay, self-hostable |

D also keeps the existing security model intact rather than bolting a second one
beside it: every participant's application is confined by *their* Ephemeral,
under permissions *they* granted, which is the property
[ADR-0012](0012-sharing-distributes-recipes.md) exists to protect.

An earlier draft of this ADR recommended A with B as an opt-in. That was wrong,
and it was wrong because it accepted the framing instead of questioning it.

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

- A **malicious session holder** — under A or B they see everything by
  construction, and the design must state that rather than pretend to mitigate
  it. Under D, end-to-end encryption reduces it to metadata, which must still be
  disclosed rather than described as "private".
- A **malicious guest** — attacking the application, the host's machine, or
  other participants through the application.
- **Invite leakage** — an invite is a capability, so treating it as a secret,
  scoping it, expiring it and revoking it are all load-bearing.
- **Prompt injection through other people's content** — a message written by a
  guest is untrusted input that may later reach a generation or repair run. This
  is the existing prompt-injection problem with a new, cheap delivery route.
- **Exposure beyond loopback** — under A, the single largest change to the
  desktop attack surface so far, and the reason hosting would be its own
  explicit decision. Option D avoids it entirely: no participant accepts inbound
  connections.
- **Revocation must be real** — ending a session must terminate access
  immediately, not merely stop issuing new invites.

## Revisit when

The open question is answered. Until then this ADR is a description of the
problem and not a licence to build.
