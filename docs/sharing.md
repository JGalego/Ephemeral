# Sharing

> **Design, not implementation.** Nothing on this page is built. It is recorded
> now because it changes assumptions the current code makes — particularly that
> there is exactly one user — and it is cheaper to know that before Phase 1 than
> after. Tracked as Phase 7 in [the roadmap](roadmap.md).

## "Share" means three different things

Collapsing them is how sharing becomes dangerous, so Ephemeral keeps them apart
and names them differently.

| | You send | They get | Risk |
|---|---|---|---|
| **Share an intent** | A sentence | Their own app, generated fresh | Lowest — nothing of yours travels |
| **Share an app** | A recipe | The same app, rebuilt on their machine, their data | Low — their Ephemeral confines it |
| **Share an instance** | An invite | Access to *your* running app and its state | Highest — a host sees everything |

The first two are covered by
[ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md). The
third is a genuinely different problem and has its own decision record,
[ADR-0013](architecture/decisions/0013-shared-instances-have-a-host.md).

## Sharing an app

The interesting property, and the reason Ephemeral can do this well:

> **You can accept an application from somebody you have no reason to trust.**

Not because the sender is vouched for, and not because anybody reviewed the
code. Because a package carries permission **requests**, never **grants** — so
your Ephemeral, on your machine, decides what it may do, and confines it either
way.

That is the whole trick. It follows directly from the permission model being
default-deny, per-principal, and enforced locally
([permissions.md](permissions.md)).

### What travels

```text
manifest.yaml     what it is, and what it wants permission to do
source/           the generated source
tests/            what proves it works
```

Identified by a content digest over the recipe
([ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md)),
so two people can confirm they have the same application.

### What never travels

Your data. Your logs. Your audit entries. Your secret values — and your secret
*names*, unless you deliberately made them part of the recipe. Your permission
grants, most importantly of all.

Nothing executable travels either. The recipient's Ephemeral builds it, which
keeps their sandbox in the loop rather than asking them to trust your build.

### Publishing to GitHub

A package is ordinary files in a normal git repository. No Ephemeral-specific
hosting, no registry, no account.

```console
$ ephemeral publish apartment-comparator-3f2a1b9c --to ./apartment-comparator
$ cd apartment-comparator && git init && git push ...

$ ephemeral install https://github.com/someone/apartment-comparator
```

The nice consequence: **the manifest is the security review.** Somebody deciding
whether to run this does not have to read the source. They read one legible file
that states, in plain language, everything the application may touch — and they
know it is not a promise, because their own Ephemeral enforces it.

That is a better review artifact than most published software offers.

### Installing something shared

```text
Apartment Comparator
Published by ana@example.com (signature verified)
Digest 3f2a1b9c…

It wants to:
  read the files in ~/Downloads/apartments          [medium]
      to compare the CSV files you select

It does not ask for: network access, the ability to run other programs,
the camera, the microphone, or your location.

Ephemeral will build this on your machine. It gets nothing until you allow it.

  [Review the source]  [Build it]  [No thanks]
```

A signature proves **who published it**. It says nothing about whether the
application is safe, and the interface must never imply otherwise — the manifest
and the sandbox are what make it safe, and a signature only says who to blame.

## Versioning

Sharing forces versioning to be real, because two people cannot discuss "the CSV
comparator" without something that names *which* one.

Every generation and every successful repair produces an immutable version,
identified by a digest. The chain is kept, so you can see what changed and go
back to one that worked. Rolling back selects an existing version; it never
mutates the current one.

The security-critical part:

> **An update that wants more than the version you approved is a permission
> decision.**

Moving to a version that requests anything the running one did not is presented
with the same prompt as any other permission, and refused by default until you
decide. Narrowing needs no prompt.

Every app store has got this wrong at some point by letting an update quietly
carry a wider permission set than the version the user approved. Ephemeral has a
permission model good enough to get it right, and would be throwing that away by
treating a version as an incrementing integer.

## Sharing an instance

This is the group-chat case — one application, one body of state, several
people. It is the demo everybody wants, and it is where the current design runs
out.

### What it breaks

**There is more than one user.** `Actor::User` is a singleton today. Somebody
joining by invite is not the person who granted the app's permissions, and the
model has no way to say so.

**The application handles other people's data.** Everything in the permission
model protects *the user* from *the application*. A shared instance also has to
protect participants from each other, and all of them from whoever runs it.

**It has to be somewhere.** A local-first application that several people use at
once is a contradiction unless something is reachable by all of them — and ports
bind to loopback by design.

### The honest framing

A shared instance is a **service with a host**, and whoever hosts it can read
everything in it. That is what hosting means. Ephemeral's job is to say so
plainly, in the invite, before anyone accepts:

> **Ana is inviting you to Group Chat, running on Ana's computer.**
>
> You will be able to: read and send messages in this room.
> You will not be able to: change the app, see other rooms, or read anything
> else on Ana's computer.
>
> Messages you send are stored on Ana's computer. Ana can read them.
>
> This invite expires in 7 days and Ana can revoke it at any time.

An invite is a capability: scoped, expiring, individually revocable, and audited
on both sides. Accepting one carries the same weight as granting a permission,
because that is what it is.

### Three planes of permission

Keeping these apart is the design. Collapsing any two is how it goes wrong.

| Plane | Governs | Decided by |
|-------|---------|------------|
| Host's app permissions | What the app may touch on the **host's** device | The host |
| Guest's app permissions | What the app may touch on the **guest's** device | The guest, on their own machine |
| Participation capability | What a guest may do **inside** the app | The host, via the invite |

The third is new, and Ephemeral should supply it rather than leaving generated
code to invent authorisation badly. A generated application must not be the
thing deciding who is allowed to use it.

### The open question

**Where does a shared instance run?** On the host's own device, on an Ephemeral
control plane, or peer-to-peer. It decides the privacy properties, the running
cost, and how much infrastructure the project takes on, so it is a product
decision rather than a technical one. The options and the recommendation are in
[ADR-0013](architecture/decisions/0013-shared-instances-have-a-host.md).

Until that is answered, this part is a description of a problem rather than a
plan.

## What has to exist first

Sharing sits on top of most of the product, which is why it is Phase 7:

| Needs | For |
|-------|-----|
| Phase 1 — runtime | Something to build and run a received recipe |
| Phase 2 — generation | Something to produce versions in the first place |
| Phase 3 — sandboxing | The confinement that makes accepting a stranger's app reasonable |
| Phase 6 — threat model | Shared instances are the largest expansion of the threat model so far and must not be built before it exists |

The versioning groundwork in
[ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md)
belongs with Phase 2, because that is when versions start being produced and
retrofitting identity onto history that was never recorded is not possible.
