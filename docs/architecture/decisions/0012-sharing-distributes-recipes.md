# ADR-0012: Sharing distributes recipes, never grants

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 7 — Sharing

## Context

"Share this app" turns out to mean three different things, and collapsing them
is how sharing becomes dangerous:

1. **Share the intent.** "Here is what I asked for." The recipient's Ephemeral
   generates its own application. Cheapest and safest, but they get a different
   application — generation is not deterministic.
2. **Share the application.** They get *this* application, rebuilt on their
   machine, with their own data and their own permission decisions.
3. **Share a running instance.** One application, several people, shared state —
   the group-chat case. Covered separately in
   [ADR-0013](0013-how-several-people-share-an-application.md), because it is a different
   problem wearing the same word.

This ADR is about (2), which is what people mean by publishing to GitHub or
sending somebody an app.

The obvious design is to send the application: manifest, source, maybe a built
image, and — since the sender already decided what it may do — its permissions,
so it "just works" on arrival. That design is wrong in a way worth being
explicit about. It makes the recipient's safety a function of the sender's
trustworthiness, and it is exactly how every "install this and click allow"
supply-chain problem works.

Ephemeral has an unusual advantage here. Because the permission model is
default-deny, per-principal, and enforced by *the recipient's* installation, a
person can accept an application from somebody they have no reason to trust —
provided grants do not travel with it.

## Decision

**A shared application is a recipe. The recipient's Ephemeral builds it, and the
recipient decides what it may do.**

The rules:

- **A package carries permission *requests*, never grants.** The manifest says
  what the application wants and why. The recipient's ledger starts empty for it,
  and every capability is decided by the recipient, with the same prompts as any
  other permission decision. This preserves "only a person decides"
  ([ADR-0003](0003-two-tier-permission-model.md)) across the sharing boundary.
- **The package is the recipe, not the binary**: manifest, generated source,
  tests, and pinned runtime inputs, identified by the version digest
  ([ADR-0011](0011-immutable-content-addressed-versions.md)). Nothing executable
  is transferred. The recipient's own Ephemeral builds and confines it.
- **The manifest is the review.** Before anything is built, the recipient is
  shown what the application will be allowed to ask for, at what risk level, and
  the sender's stated reasons. Accepting is a decision, not a formality.
- **A package is ordinary files.** `ephemeral publish` writes a directory that is
  a normal git repository — readable, diffable and reviewable on GitHub by
  someone who has never run Ephemeral. The manifest doubles as a
  human-legible statement of what the code may do, which is a far better
  security review artifact than reading the source.
- **Nothing local travels.** Not the app's data, not its logs, not its audit
  entries, not secret values, not secret *names* the user did not mark as part
  of the recipe. Publishing is explicit about what leaves the machine and asks
  before it does.
- **Provenance is optional and honest.** A package may be signed to prove *who
  published it*. Signing says nothing about whether the application is safe, and
  the interface must not imply otherwise. The manifest and the sandbox are what
  make it safe; a signature only says who to blame.

## Alternatives considered

### Ship the built image

The recipient runs bit-for-bit what the author ran, so "works on my machine" is
solved. Rejected: it transfers an executable, which makes the recipient's safety
depend on trusting the sender and on the sender's build environment. It also
defeats the sandbox's purpose — Ephemeral's whole posture is that code is
untrusted and gets built and confined locally.

### Ship the app *with* its permissions, so it works on arrival

The frictionless experience, and what an app store does. Rejected outright. It
means the sender decides what runs on the recipient's machine, and it converts
"only a person decides" into "the person who decided was somebody else". The
friction here is the feature.

### Share only the intent and let the recipient regenerate

Beautifully simple, nothing but a string travels, and it fits the product thesis
that intent is the durable object. Rejected as the *only* mechanism because
generation is not deterministic: the recipient gets a different application,
which is useless when the point is "use the thing I built", and impossible to
reason about when the point is "review what I am running". Kept as a supported
mode — it is the safest one, and the right default for a casual "you should try
this".

### A central Ephemeral registry

Discovery, ratings, a canonical namespace. Rejected for now on two grounds: it
requires infrastructure and moderation the project has no business taking on at
this stage, and a curated registry implies a safety judgement Ephemeral is not
in a position to make. Git hosting already solves distribution, and the trust
model deliberately does not depend on the distributor.

## Consequences

### What this makes easier

Accepting an application from a stranger becomes a reasonable thing to do, which
is the property that makes sharing viable at all. Publishing to GitHub needs no
Ephemeral-specific infrastructure. Review is possible for people who cannot read
the source, because the manifest states the capabilities in plain language.
Reproducibility is testable: the digest either matches or it does not.

### What this makes harder

The recipient must build, which takes time and needs whatever runtime the
application declares. They are asked permission questions the sender already
answered, which will feel repetitive to anyone who has not thought about why.
And an application that depended on something the sender had and the recipient
does not will fail on arrival — visibly, which is better than the alternative,
but it is still a failure.

### What we are accepting

Sharing is slower and more interactive than sending a file. We are choosing that
deliberately, and the interface's job is to make the questions feel like
informed consent rather than nagging.

## Security implications

This is the decision that makes sharing safe rather than a supply-chain
liability.

- Grants never travel, so a malicious sender cannot pre-authorise anything.
- No executable travels, so a compromised sender cannot ship a backdoored
  binary; they can ship malicious *source*, which the recipient's sandbox and
  permission decisions still confine.
- The digest lets a recipient verify they received what was offered, and lets two
  recipients verify they received the same thing.
- Publishing is an outbound data-flow decision, so it is gated and audited like
  any other: the user is told exactly what will leave the machine.
- Signatures are scoped honestly to authorship. Presenting a signature as a
  safety endorsement would be the single easiest way to undo all of the above.

## Revisit when

- Reproducible builds are strong enough that shipping an artifact adds
  verifiable value the recipe does not.
- Discovery becomes a real user need that git hosting genuinely cannot serve.
