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
| **Share a session** | An invite | Their own copy of the app, plus shared state | Highest — other people see what you put in it |

The first two are covered by
[ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md). The
third is a genuinely different problem and has its own decision record,
[ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md).

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

## Sharing a session

This is the group-chat case — one application, one body of state, several
people. It is the demo everybody wants, and it is where the current design runs
out.

### What it breaks

**There is more than one user.** `Actor::User` is a singleton today. Somebody
joining by invite is not the person who granted the app's permissions, and the
model has no way to say so.

**The application handles other people's data.** Everything in the permission
model protects *the user* from *the application*. A shared session also has to
protect participants from each other, and all of them from whoever holds the
shared state.

**The state has to be somewhere.** Several people cannot share something unless
it is reachable by all of them — and ports bind to loopback by design.

### The honest framing

Whoever holds the shared state can read it. Ephemeral's job is to say so
plainly, in the invite, before anyone accepts — naming exactly who that is,
because "stored on Ana's computer, where Ana can read them" is a different
sentence from "shared directly between participants":

> **Ana is inviting you to Group Chat.**
>
> You will be able to: read and send messages in this room.
> You will not be able to: change the app, or see other rooms.
>
> The app runs on your device, under permissions you grant. Messages are shared
> with everyone in the room.
>
> This invite expires in 7 days and Ana can revoke it at any time.

An invite is a capability: scoped, expiring, individually revocable, and audited
on both sides. Accepting one carries the same weight as granting a permission,
because that is what it is.

### Three planes of permission

Keeping these apart is the design. Collapsing any two is how it goes wrong.

| Plane | Governs | Decided by |
|-------|---------|------------|
| Each participant's app permissions | What their copy may touch on **their own** device | That participant |
| Session membership | Who is in the room at all | Whoever holds the session, via invites |
| Participation capability | What a member may do **inside** the app | The inviter, via the invite |

The third is new, and Ephemeral should supply it rather than leaving generated
code to invent authorisation badly. A generated application must not be the
thing deciding who is allowed to use it.

### The app and the session are different things

The instinct is to ask "where does the shared app run?" — and that conflates two
separable things.

The **application** needs no shared home. It is distributable as a recipe, and a
recipe the recipient rebuilt is *theirs*: it runs on their device under
permissions they granted, and it survives the author deleting theirs.

What needs a shared home is the **session** — the conversation. That is a much
smaller problem, because a relay moves data rather than code, so it can be
blinded and self-hosted.

### What it looks like

Ana wants a group chat.

**She asks for it.**

```console
$ ephemeral create "a group chat for me and my flatmates"
$ ephemeral share flat-chat --invite --expires 7d
```

Sharing is a deliberate act with its own switch. Applications are not shareable
by default.

**Bob has Ephemeral.** The link opens there, and before anything runs:

> **Ana is inviting you to Flat Chat.**
>
> Ephemeral will build this on your device. It wants to save messages in
> `~/Ephemeral/flat-chat`. It does not want your camera, your location, or the
> rest of your files.
>
> You will be able to read and send messages in this room. You will not be able
> to change the app.
>
> Expires in 7 days. Ana can revoke it.

He approves, his copy builds, he is in — **his copy, his permissions**. Ana's
decisions did not travel.

**Carla is on her phone with no Ephemeral.** The same link opens a web page. She
can chat, and she is told why it is not the same:

> You are joining as a guest. This runs in your browser rather than on your
> device, so Ephemeral cannot protect it the way it protects members.

**Nobody is the server.** Messages sync between members' copies through a relay
that cannot read them. Ana closing her laptop changes nothing.

**Ana deletes her copy.** Bob's still works — it was always his, and the
conversation survives.

**Ana revokes Carla's invite.** Carla is out immediately, mid-session. Not "no
new invites": out.

### Member and guest

| | Member | Guest |
|---|---|---|
| Needs Ephemeral | Yes | No — a browser |
| Where the app runs | Their own device | Their browser |
| Who decided its permissions | They did | Nobody: it has no local access to give |
| Confined by Ephemeral's sandbox | Yes | **No**, and it says so |
| Survives everyone else leaving | Yes | Only while a member remains |

Requiring Ephemeral everywhere is the price of the guarantees, and it is the
steepest onboarding cost in the product — which is exactly why the guest tier
exists. A guest cannot be given a member's guarantee, and the reason is
fundamental: **whoever serves the browser code can break the encryption.**

### Metadata is a requirement, not a footnote

Encrypting contents is not enough. Who talks to whom, and when, is often more
sensitive than what was said. The invariant:

> **The relay never learns more than a participant already knows.**

So the group operates its own relay — a member's device, or something they
self-host — making the operator somebody who can already read the messages.
Per-room identities stop anything correlating across rooms, sealed sender hides
who sent what, and padding blunts timing analysis.

There is no other kind of relay. Ephemeral does not run one, and there is no
opt-in to a third party, because "opt-in with a named cost" is how a default
arrives: it would be the easy path for every group that found self-hosting
inconvenient, and within a year the honest sentence would be "Ephemeral sees
who talks to whom, unless you configure otherwise."

**This costs reach, and that is the trade.** A group where nobody will keep a
device on does not get a shared session. Ephemeral says so when somebody tries,
rather than in a footnote.

What cannot honestly be promised is removing metadata entirely. Some
intermediary has to exist — peer-to-peer needs rendezvous, offline delivery
needs store-and-forward, and mobile needs a push service. The intermediary can
be made blind; it cannot be made absent.

## What works today

`ephemeral publish` writes an ordinary directory — `git init` it and push it
anywhere. `ephemeral install` shows the recipient what it is and what it will
want, and installs nothing until they say so.

```console
$ ephemeral publish csv-comparator ./csv-comparator
$ ephemeral install ./csv-comparator            # shows it; changes nothing
$ ephemeral install ./csv-comparator --accept   # takes it, with no permissions
```

What travels: the manifest, the source, its tests, and the version digest.
What does not: the data, the logs, the audit record, the lifecycle history, the
sender's own installation id, their tags, their retention choice, the names of
any settings they held — and **every permission decision they made**. The
recipient's copy arrives with nothing and is asked about each request
separately, with the reason the application itself gave.

Two publishes of the same application produce the same package, so two people
can check they received the same thing.

Shared *sessions* — several people using one running application — are
[ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md)
and are not built.

### What would reopen this

[ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md)
is **accepted**. Two things would justify revisiting it: a relay design where
the operator provably *cannot* observe metadata rather than merely finding it
inconvenient, or evidence that the reach cost is what stops people using sharing
at all — with the evidence, rather than pre-emptively on the assumption that it
will be.

Nothing here is built. Sharing is Phase 7, and
[the threat model](../SECURITY.md#threat-model) comes first — this is the
largest expansion of it since the product began.
