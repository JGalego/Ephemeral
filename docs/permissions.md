# Permissions

Ephemeral has two permission systems. Understanding why there are two, rather
than one, is most of understanding the product's security model.

Rationale and rejected alternatives:
[ADR-0003](architecture/decisions/0003-two-tier-permission-model.md).

## Why two

Ephemeral needs broad authority to do its job: install runtimes, drive Docker,
execute processes, read directories you point it at, reach the network to call a
model provider.

A generated application needs almost none of that. The CSV comparator needs to
read two files.

The dangerous and entirely natural design is one permission set, where generated
apps run "as Ephemeral". That fails the moment any generated app is malicious or
is steered by prompt injection, because every app then holds every capability
the product holds. It also makes your consent meaningless: you approved Docker
access for *Ephemeral*, not for a program a language model wrote ninety seconds
ago.

So there are two, and they are separate types that cannot be substituted for one
another.

| | Who holds it | What it governs |
|---|---|---|
| **Meta-permission** | Ephemeral itself | Docker, installing runtimes, executing processes, the network, the keychain, the camera |
| **Application permission** | One generated app | Exactly what that app may touch |

## The rules

### 1. No inheritance, in either direction

A grant names exactly one principal. Ephemeral holding `filesystem.read(~/**)`
grants a generated app **nothing**. Granting an app the camera gives Ephemeral
nothing either. One app's grants say nothing about another's.

```console
$ ephemeral grant ephemeral read:'~/**' --why "so it can open files you point it at"
$ ephemeral permissions my-app-3f2a1b9c
Allowed to
  Nothing. Permissions have to be granted one at a time.
```

### 2. Default deny

An absent grant is a refusal, not an unknown. Only an explicit, unexpired,
unrevoked `Allow` naming that principal and covering that request permits
anything.

### 3. An explicit denial wins

Whenever it was recorded, and whatever else exists. A denial of a broad scope
covers everything inside it, so "never let this app touch my home directory"
means what it says.

### 4. Only a person decides

`PermissionLedger::decide` refuses any actor but `Actor::User`. The generation
agent cannot grant a permission — to itself or to an app it wrote — no matter
what a model was persuaded to output. Neither can the orchestrator, the runtime
or the system.

This is the structural defence against prompt injection. It holds because it is
checked in code, not requested in a prompt.

### 5. Ephemeral's permission is a ceiling, not a source

For an app to do something, **both** it and Ephemeral must be permitted. So
revoking a meta-permission disables that capability for every app at once,
whatever their manifests say.

```console
$ ephemeral revoke ephemeral camera
Revoked. Ephemeral can no longer use the camera.
```

Every app that could use the camera now cannot, and each is told why: *"Ephemeral
itself has not been allowed to use the camera, so no app can do this yet."*

## Scopes

A permission without a scope is not a permission. `PathScope` and `HostScope`
are what make least privilege expressible.

### Paths

| Written | Means |
|---------|-------|
| `~/Downloads/report.csv` | exactly that one path |
| `~/Downloads/apartments/**` | that directory and everything in it |
| `/etc/hosts` | exactly that one path |
| `C:/Users/ana/**` | that directory and everything in it |

Granting `~/Downloads` alone does **not** grant anything *in* `~/Downloads`.
That is deliberate: the two are different permissions and are written
differently.

Containment is decided segment by segment, never by string prefix:

- `/home/user/docs/**` does **not** cover `/home/user/docs-private`
- `..` is refused at parse time rather than resolved, so a scope always reads as
  what was approved
- different roots never contain each other

The check is lexical, because the domain layer performs no host I/O and
therefore cannot see through symbolic links. The runtime resolves links and
applies the same rule again before mounting anything. Both checks are required;
neither is sufficient alone.

### Hosts

| Written | Means |
|---------|-------|
| `api.example.com` | that host, any port |
| `api.example.com:443` | that host, that port only |
| `*.example.com` | any subdomain, but **not** `example.com` itself |
| `*` | anywhere — flagged as unrestricted |

Wildcards respect the dot boundary, so `*.example.com` does not cover
`notexample.com` or `example.com.attacker.net`.

## Risk

Every permission carries a risk level, and the same capability is riskier for a
generated app than for Ephemeral — the code holding it was written by a model
minutes ago and reviewed by nobody.

| Level | Meaning |
|-------|---------|
| `low` | little that could go wrong |
| `medium` | worth reading before deciding |
| `high` | could expose personal data or meaningfully widen what code can reach |
| `critical` | could undermine the other protections around this app or device |

`high` and `critical` require an explicit confirmation rather than a
default-highlighted button.

Two worth calling out:

- **Unrestricted egress** (`net:*`) is `high` for an app. It is the permission
  that turns a read permission into a data-exfiltration permission, and it says
  so: *"It can send data anywhere on the internet, including data it read from
  your files."*
- **Running other programs** (`execute`) is `critical` for an app, because it
  weakens the value of every other limit on it.

## Asking

A permission request is a structured `PermissionPrompt`, not a string. It
carries answers to the five questions a person actually needs:

> **Apartment Comparator wants to read the files in `~/Downloads/apartments`.**
>
> It needs this to compare the CSV files you selected.
>
> If you allow it: it can read what is at `~/Downloads/apartments`. It cannot
> change those files, and it cannot see anything else on this device.
>
> You can take this back at any time from the app's page.
>
> \[Allow]  \[Deny]

There is deliberately no free-form "message" field, so no interface can
substitute its own vaguer wording. That is what makes *"Allow filesystem
access?"* unshippable rather than merely discouraged.

## Revocation

Revocation errs **broad**, deliberately.

A grant is revoked if it covers the named permission *or* is covered by it:

- revoking `~/Downloads/**` withdraws a wider `~/**` grant, because leaving
  something in place that still permits what you asked to stop is the dangerous
  failure;
- revoking `~/**` withdraws the narrower grants inside it, so "stop reading my
  home directory" does not leave a surviving sub-grant.

Scopes cannot be partially subtracted, so the alternative to over-revoking is
under-revoking — and only one of those fails safe. If you want the narrower
access back, grant it again.

Revoking marks grants rather than deleting them, so "this was allowed on Monday
and revoked on Tuesday" stays answerable.

## From the command line

```console
# What Ephemeral itself may do
$ ephemeral permissions ephemeral

# What one app may do
$ ephemeral permissions apartment-comparator-3f2a1b9c

# Allow something, recording why
$ ephemeral grant apartment-comparator-3f2a1b9c read:'~/Downloads/apartments/**' \
    --why "to compare the CSV files I selected"

# Take it back
$ ephemeral revoke apartment-comparator-3f2a1b9c read:'~/Downloads/apartments/**'
```

The word `ephemeral` in the principal position means the product itself. Full
syntax is in `ephemeral grant --help`, and a typo is always an error listing
what would have worked — a permission grammar that silently ignored one would
grant something other than what you typed.

## What is enforced today

Phase 0 delivered the **model**: the types, the ledger, the decision rules and
the tests that state them. Phase 1 connects it to a running process — a granted
scope becomes a mount, an inbound port becomes a loopback binding, and an
application with an empty ledger gets a container that can see nothing of yours.
[sandbox.md](sandbox.md) is the list of exactly what that buys.

`ephemeral review` walks through everything an application has asked for and
not been given, one question at a time. It answers all five questions above, and
holds two rules: nothing is granted without an answer — no default, no "allow
all", and no timeout — and a high-risk permission takes the word `allow` rather
than a keystroke, so a `y` habit formed on easy questions does not carry over to
the one that matters. Run without a terminal, it prints what it would ask and
decides nothing.

Two things are still model rather than enforcement. A **hostname allow-list**
cannot be applied by Docker, so an application granted one refuses to start
rather than running with more access than its owner allowed. **Device access** —
camera, microphone, location — is recorded and shown but not yet mediated, and
arrives with the platform adapters in Phase 3. See [roadmap.md](roadmap.md).

The security suite in `crates/ephemeral-core/tests/security.rs` is the list of
promises this page makes, written as executable assertions. If one of them
fails, that is a vulnerability rather than a broken test.
