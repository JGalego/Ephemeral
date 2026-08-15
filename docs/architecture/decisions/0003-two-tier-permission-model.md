# ADR-0003: Two separate permission systems, with no inheritance

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation (model), 3 — Permissions (enforcement)

## Context

Ephemeral needs broad authority to do its job: install runtimes, drive Docker,
execute processes, read directories the user points it at, reach the network to
call a model provider. A generated application needs almost none of that. The
CSV comparator needs to read two files.

The dangerous and entirely natural design is a single permission set: Ephemeral
asks the user once, and generated apps run "as Ephemeral". That design fails the
moment any generated app is malicious or is steered by prompt injection, because
every app then holds every capability the product holds. It also makes the
user's consent meaningless — they approved Docker access for *Ephemeral*, not
for a program a language model wrote ninety seconds ago.

There is a second force: the operating systems already have permission systems.
macOS TCC, Android runtime permissions, iOS entitlements, and the Windows
capability model all mediate camera, microphone, location, contacts and
filesystem access. A product that invents its own parallel model and asks the
user for "camera permission" it cannot actually enforce is lying to them.

## Decision

**Two permission spaces, modelled as two distinct types, with no inheritance
between them.**

1. **Meta-permissions** govern what Ephemeral itself may do: install
   dependencies, execute processes, use and install Docker, pull images, read
   and write given paths, access the network, read environment variables, use
   the OS keychain, create shortcuts and notifications, access camera,
   microphone, location, contacts, calendar, browser data and external devices,
   and update itself.

2. **Application permissions** govern what one generated app may do, scoped as
   narrowly as practical: specific paths with specific modes, outbound network
   with an allow-list, process execution, and each device capability
   individually.

The rules:

- **No inheritance, in either direction.** A grant names exactly one principal
  (`Ephemeral`, `App(id)`, or a future `Plugin(id)`). Ephemeral holding
  `filesystem.read(~)` grants an app nothing.
- **Default deny.** Only an explicit, unexpired, unrevoked `Allow` for that
  principal and that permission permits an operation.
- **Explicit deny wins.** A `Deny` cannot be overridden by a later `Allow` from
  a non-user actor.
- **Only a user grants.** The generation agent cannot grant a permission — to
  itself or to an app it generated. This is enforced in the core, not in a
  prompt, and it is the primary structural defence against prompt injection.
- **Ephemeral's meta-permission is necessary but not sufficient.** For an app to
  read a path, *both* Ephemeral and the app must be permitted. A revoked
  meta-permission disables the capability product-wide.
- **The OS is the source of truth** where it has one. The platform adapter
  mirrors OS state into the ledger; it never fabricates a grant the OS has not
  given.

Every decision is recorded as a `Grant` with subject, permission, decision,
granting actor, timestamp, optional expiry and revocation state, and every
decision is mirrored into the audit log.

## Alternatives considered

### One permission set, apps run as Ephemeral

Simplest to build and to explain, and it is what a naive implementation
produces. Rejected outright: it makes every generated app as powerful as the
product, destroys the meaning of user consent, and provides no defence
whatsoever against a malicious or injected app. This is the failure mode the
product exists to avoid.

### Apps inherit a subset of Ephemeral's permissions, narrowed per app

Superficially attractive — it guarantees an app can never exceed the product,
and it saves a prompt. Rejected because inheritance is the wrong default and
inverts the safe direction of error. Under inheritance, *forgetting* to narrow
yields maximum privilege; under default-deny, forgetting yields none. It also
makes an app's effective authority a function of Ephemeral's current grants,
which means widening a meta-permission silently widens every existing app.
Ephemeral's grant remains a *ceiling* — it is checked — but it is never a
*source*.

### Capability tokens / object-capability model throughout

Theoretically the strongest option: unforgeable references, no ambient
authority, natural delegation and revocation, and a clean answer to app-to-app
composition. Rejected for now on grounds of user comprehension and scope. The
permission UI must answer "what is asking, what does it want, why, what happens
if I allow, can I revoke" to a non-technical user, and capability tokens are
harder to render honestly in those terms. We keep the door open: the ledger's
principal/permission pairs map onto capabilities cleanly, and app-to-app
composition (ARCHITECTURE §12) is expected to introduce explicit capability
contracts.

### Defer to the OS sandbox alone

Let macOS App Sandbox, Android permissions and Windows AppContainer do the work.
Rejected because those systems scope permissions to *the installed application*
— to Ephemeral — and have no concept of a per-app principal created at runtime.
They cannot express "this generated app, and only this one, may read this
directory". We integrate with them for the capabilities they own, and we need
our own layer for the per-app dimension they cannot represent.

## Consequences

### What this makes easier

Least privilege becomes the structural default rather than a discipline.
Revocation is meaningful and per-app. The audit log answers "what was this app
allowed to do, and who allowed it" precisely. Prompt injection cannot escalate
privilege, because the agent is not an actor that can grant.

### What this makes harder

Two systems to model, present and test. More prompts for the user, which we must
counter with genuinely good permission UX rather than by reducing prompts. Every
enforcement point must ask the right question about the right principal — a
check that names the wrong principal is a vulnerability, so enforcement points
need review and tests.

### What we are accepting

Some redundancy between what the OS enforces and what we enforce, and the
possibility of drift between the two if a user changes an OS setting behind our
back. The platform adapter's mirroring job exists to keep that drift visible.

## Security implications

This ADR *is* the security model's foundation. The invariants it establishes —
no inheritance, default deny, only-a-user-grants — are covered by the
`tests/security/` layer, and a change that weakens any of them should be
treated as a vulnerability rather than a refactor.

## Revisit when

- App-to-app composition needs delegation the ledger cannot express (→ likely a
  capability-contract ADR that supersedes part of this one).
- A platform introduces a per-principal runtime permission model we could adopt
  directly.
