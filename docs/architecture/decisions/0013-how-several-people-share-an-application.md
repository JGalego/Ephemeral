# ADR-0013: How several people share one application

- **Status:** **proposed.** The shape below is agreed; what remains open is who
  operates a relay, which is an infrastructure commitment. Nothing is built.
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 7 — Sharing

## Context

[ADR-0012](0012-sharing-distributes-recipes.md) covers giving somebody an
application: they rebuild it, with their own data and their own permission
decisions. That is safe precisely because nothing is shared at runtime.

It does not cover the thing people most want to demonstrate:

> "Build me a group chat, and let my friends join with a link."

Here there is one application, one body of state, and several people. That
breaks the current design in three ways:

**There is more than one user.** `Actor::User` is a singleton today — "the
person at this machine", unnamed. Somebody joining by invite is not the person
who granted the application's permissions.

**The application handles other people's data.** The permission model protects
*the user* from *the application*. Sharing also has to protect participants from
each other, and all of them from whoever holds the shared state.

**The state has to be somewhere.** Several people cannot share something unless
it is reachable by all of them, and ports bind to loopback by design.

## Decision

**Separate the application from the session.**

The application does not need a shared home. It is already distributable as a
recipe, and a recipe the recipient rebuilt is *theirs*: it runs on their device
under permissions they granted, and it survives the author deleting theirs.

What genuinely needs a shared home is the **session** — the conversation. That
is a much smaller problem, because a relay moves data rather than code.

### Two tiers, labelled

Requiring Ephemeral on every device is the price of the guarantees, and it is a
real cost to reach. So there are two ways to take part, and the difference is
stated rather than hidden — the same way `NativeRuntime` admits it is less
isolated than a container ([ADR-0005](0005-docker-first-runtime-abstraction.md)).

| | Member | Guest |
|---|---|---|
| Needs Ephemeral | Yes | No — a browser |
| Where the app runs | Their own device | Their browser, served by a member or the relay |
| Who decided its permissions | They did | Nobody: it has no local access to give |
| Confined by Ephemeral's sandbox | Yes | **No**, and the interface says so |
| Survives every other participant leaving | Yes | Only while some member remains |

A guest cannot be given a member's guarantee, and the reason is fundamental
rather than an implementation gap: **whoever serves the code can break the
encryption.** A relay that ships the browser code holding the keys can ship
backdoored code instead. This is the standing objection to browser-delivered
end-to-end encryption; subresource integrity and code signing do not close it,
because the same origin serves the checker. The tier is therefore presented as
weaker, and "get Ephemeral for the full thing" is the honest call to action.

### Metadata is a requirement, not a disclosure

Content encryption alone is not enough. Who talks to whom, and when, is
frequently more sensitive than what was said. The invariant:

> **The relay never learns more than a participant already knows.**

Which means:

- **The group operates its own relay by default** — a member's device or one
  they self-host. Then the operator is somebody who can already read the
  messages, so metadata reveals nothing additional. This is the default, not a
  fallback.
- **Per-room identities**, so nothing correlates a person across rooms.
- **Sealed sender**, so a relay sees "a message for this room" rather than who
  sent it.
- **Padding and batching**, to blunt size and timing analysis.

Using a third-party relay is then an explicit opt-in with a named cost, rather
than the default with a footnote in the documentation.

What cannot be honestly promised is the elimination of metadata while any
intermediary exists. The intermediary can be made blind; it cannot be made
absent — see [the alternatives](#alternatives-considered).

### Identity

Each installation holds a keypair, generated on first use and kept in
platform-native secure storage. A participant *is* a public key, with a display
name that is a label rather than an identity. No accounts, no directory, nothing
central.

### An invite is a capability

Signed, scoped, expiring by default, individually revocable, and audited on both
sides. Revocation must take effect on a live session — ending access
immediately, not merely declining to issue new invites. Accepting one carries
the weight of a permission decision, because that is what it is.

### Three planes of permission, kept apart

| Plane | Governs | Decided by |
|-------|---------|------------|
| Each participant's app permissions | What their copy may touch on **their own** device | That participant |
| Session membership | Who is in the room at all | Whoever holds the session, via invites |
| Participation capability | What a member may do **inside** the app — send, invite, moderate | The inviter, via the invite |

The third is new. Ephemeral supplies the primitive so that generated code does
not reinvent authorisation badly; a generated application must not be the thing
deciding who may use it.

### Sharing is explicit and revocable

An application is not shareable by default. Starting a shared session is its own
action, with its own consent, audit trail, and off switch that ends every
participant's access at once.

## Alternatives considered

### Host it on the author's device; guests connect

Guests need no install, which is the best reach of any option. Rejected as the
model: the application dies when the author closes their laptop, the author sees
everything by construction, it needs inbound connections — the largest change to
the desktop attack surface so far — and **a phone can never host**, having no
container runtime and no background residency. A group of phone users would have
nobody able to run it.

### Host it on an Ephemeral control plane

Always available, no NAT problem, and mobile needs a control plane anyway
([ADR-0007](0007-mobile-control-plane.md)). Rejected as the model because
everyone's data leaves every device, and it makes the project operate
multi-tenant execution of untrusted generated code — a high-value target holding
many users' data. Local-first would stop being true for the feature people demo
most.

### Pure peer-to-peer

The most attractive on privacy: no relay, no host, nobody positioned to observe
anything. An earlier draft of this ADR dismissed group membership as unsolved,
which was **wrong and out of date** — MLS ([RFC 9420](https://www.rfc-editor.org/rfc/rfc9420.html))
is a standardised answer to group key agreement and membership changes.

Rejected anyway, because it does not remove infrastructure and it loses on the
two properties that matter most here:

- **Rendezvous.** Peers behind NAT cannot find each other unaided. Hole-punching
  fails routinely on mobile networks, and the fallback is a TURN relay that
  carries the traffic — so there is a relay regardless, just for the hard cases.
- **Offline delivery.** If one person is asleep, a message waits until both are
  online at once. For a chat application that is fatal, and every fix is
  store-and-forward at a peer or a relay — something holds the data either way.
- **Mobile push.** Phones cannot hold persistent connections. Waking an
  application requires APNs or FCM, third parties that observe metadata whatever
  else is done.

The decision above is therefore not "relay instead of peer-to-peer" but **direct
connections where they work, a blind relay where they do not** — which is what
the alternative reduces to in practice once offline delivery and mobile are
required.

## Consequences

### What this makes easier

The application survives anybody leaving, because everyone owns their copy.
Participation works on every platform. The existing security model stays intact
rather than gaining a second one beside it: each participant's application is
confined by *their* Ephemeral under permissions *they* granted.

### What this makes harder

Full participation requires installing Ephemeral, which is the steepest
onboarding cost in the product and the reason the guest tier exists at all.
Generated applications must be written against a shared-state primitive rather
than doing their own networking — a real constraint on what a model may produce,
though a much narrower one than asking it to implement conflict-free
replication itself.

### What we are accepting

A guest gets materially weaker guarantees than a member, stated plainly rather
than smoothed over. Some intermediary exists, and it can observe some metadata
even when blinded — which is why the default is that the intermediary is the
group itself.

Mobile coverage is **borrowed rather than intrinsic**: a mobile member's
application executes on the control plane ([ADR-0007](0007-mobile-control-plane.md)),
not on the handset. Participation works everywhere; "runs on your own device"
does not, and the interface must not claim otherwise.

## Security implications

The largest expansion of the threat model since the product began. It must not
be built before [the threat model](../../../SECURITY.md#threat-model) exists.
New concerns:

- **A malicious relay operator** — mitigated by content encryption and the
  metadata measures above, and by the default that the operator is a
  participant.
- **A malicious member** — sees all session content by construction; the design
  states this rather than pretending to prevent it.
- **Code delivery to guests** — whoever serves the browser bundle can subvert
  it. Not solvable within the browser; handled by labelling the tier honestly.
- **Invite leakage** — an invite is a capability, so scoping, expiry and real
  revocation are load-bearing rather than conveniences.
- **Prompt injection through other people's content** — a message written by
  another participant is untrusted input that may later reach a generation or
  repair run. The existing problem with a new and very cheap delivery route.

## Revisit when

- The relay-operation question is answered, at which point this can be accepted.
- Browser code-integrity attestation becomes real enough to narrow the gap
  between the two tiers.
