# Roadmap

Ephemeral is built in phases. A phase is not finished because code exists for
it — it is finished when the previous phase demonstrably works. This page is the
honest record of where that line currently sits.

## Where we are

**Phase 4 — Desktop.** Complete: everything the terminal does can be done in the
window, through one implementation both of them call. Phase 3 before it made the
[permission model something the product consults](security/enforcement.md)
rather than describes, and Phase 2 was
[demonstrated rather than asserted](#not-a-claim-this-time-it-was-run).

Phase 5 — Cross-platform — is next, and most of it exists: what is left there is
signing, an installable iOS build, and a run on a physical device.

### Done

| | |
|---|---|
| Repository, licence, contribution guide, security policy | ✅ |
| [ARCHITECTURE.md](../ARCHITECTURE.md) and nineteen [ADRs](architecture/decisions/) | ✅ |
| CI: format, lint, docs, tests on Linux/macOS/Windows, supply chain | ✅ |
| One-command development bootstrap | ✅ |
| `ephemeral-core`: identity, actors, errors | ✅ |
| Lifecycle state machine — 20 states, 31 events, total and actor-authorised | ✅ |
| Both permission systems and the ledger | ✅ |
| Versioned application manifest | ✅ |
| Retention policies | ✅ |
| Hash-chained audit log with redaction on write | ✅ |
| Storage layout and application store | ✅ |
| Security invariant test suite | ✅ |
| `ephemeral-cli` — the same domain model, driven from a terminal | ✅ |
| `ephemeral doctor` — environment diagnostics | ✅ |
| Reference documentation | ✅ |
| `ephemeral-runtime` — the sandbox, and the Docker implementation of it | ✅ |
| `ephemeral run`, `stop`, `pause`, `resume` | ✅ |
| Orphan cleanup — `ephemeral cleanup`, and `doctor` reporting what is left over | ✅ |
| `ephemeral status` — crash, health and clean-exit detection against the record | ✅ |
| `ephemeral logs` shows the application's own output, not only its history | ✅ |
| `ephemeral watch` — the supervisor: crash detection and wall-clock limits | ✅ |
| Building an image from an application's own source | ✅ |

Phase 0 is complete. Everything that does not need a runtime or a model provider
works end to end: ask for an application, inspect it, move it through its
lifecycle, grant and revoke permissions, read the audit trail, delete it,
restore it, purge it.

```console
$ ephemeral create "compare these two CSV files and show me what's different"
$ ephemeral grant <app> read:'~/Downloads/apartments/**' --why "to compare them"
$ ephemeral inspect <app>
$ ephemeral audit
```

Phase 1 has the sandbox: the `Runtime` trait, the Docker implementation
([ADR-0005](architecture/decisions/0005-docker-first-runtime-abstraction.md),
[ADR-0014](architecture/decisions/0014-drive-docker-through-its-cli.md)),
detection with a remedy, container lifecycle, approved mounts, loopback port
exposure, resource limits, logs, health inspection and teardown.

The confinement is decided by pure functions, so what it holds is a set of
tests rather than a claim: capabilities dropped, no network unless granted,
read grants mounted read-only, ports on 127.0.0.1, non-root, no whole-root
mount, and a refusal rather than a weaker substitute when a control cannot be
applied.

`NativeRuntime` is **deliberately not built** — see
[ADR-0015](architecture/decisions/0015-defer-the-native-runtime.md). The version
that could be built without new dependencies in the trust base would be a
sandbox in name only, and nothing generated needs it yet.

**Done when:** an application can be built from its own source, run, stopped,
archived, restored and deleted through the CLI. The build path exists and is
tested; what it has no source to build is supplied by Phase 2, so this closes
there rather than here.

## What comes next

### Phase 2 — Generation

Done so far:

| | |
|---|---|
| Content-addressed versions, and the permission delta between two ([ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md)) | ✅ |
| `AgentProvider`, with model output as validated proposals rather than commands | ✅ |
| The deterministic mock provider, and a CI job that keeps generation offline | ✅ |
| The bounded plan → generate → build → test → repair loop | ✅ |
| `ephemeral generate` — an application reaching `Ready` without anybody writing code | ✅ |
| `ephemeral review` — the permission prompt, finally reachable | ✅ |
| Regenerating, with a widening update's grants withdrawn rather than inherited | ✅ |
| A real provider (`--provider anthropic`), behind the same trait | ✅ |
| Prompts and reply parsing shared by every provider, so two cannot drift apart | ✅ |
| OpenAI's wire format (`--provider openai`), which reaches everything that copied it | ✅ |
| A local provider, which is the only real answer to "the intent leaves the machine" ([ADR-0019](architecture/decisions/0019-openai-compatible-and-a-local-model.md)) | ✅ |

Everything since, which was one story told in four rows:

| | |
|---|---|
| Each version's source kept, so a recorded version can actually be restored | ✅ |
| Returning an application to a version it used to be, in the domain model | ✅ |
| `ephemeral rollback`, with the grants a widening rollback would inherit withdrawn | ✅ |
| Rolling back from the window, through the same operation the terminal calls | ✅ |

**Done when:** the CSV comparator can be built from a natural-language request
end to end, with CI exercising the whole journey against the mock provider and
never calling a real model.

### Not a claim this time: it was run

Every row above is done, and CI runs the journey from a sentence to a ready
application on every commit — but against a build step that records what it was
given rather than a container, because CI has no daemon. *Built*, the word this
phase turns on, was the one thing no machine had watched happen. On 2026-08-22
somebody sat down with a daemon and a credential and watched it.

**With the mock provider and real Docker.** `create` → `generate` → a real
`docker build` → `ephemeral-compare-these-two-csv-5ce0e03f:8a5426d900da` → `run`
in the sandbox, which reported `added: flat-c / removed: flat-b / changed:
flat-a` from two CSV files. The container ran as uid 65534, with a read-only
root filesystem, every capability dropped, no network, and the granted directory
mounted read-only — the confinement the argument-vector tests assert, observed
this time rather than computed. `archive`, `restore`, `delete`, `restore` and
`purge` were walked through afterwards, which is Phase 1's definition of done
finally being watched end to end.

**With a real model.** `ephemeral generate --provider anthropic` planned, wrote
a 90-line application with a 101-line test suite, built it and passed its tests
on the first attempt — 1,225 tokens in, 6,565 out, no repair round. It ran in
the sandbox holding *no* permissions at all and printed a correct unified diff.
The credential appears nowhere under `EPHEMERAL_HOME`: not in the manifest, not
in the audit record, not in a recorded argument vector.

**What that turned up.** A rejected API key was reported perfectly — "anthropic
failed: API key is invalid", the provider's own words, from the one module CI
cannot exercise — and then left the application **stranded in `Generating`**. A
failure before any code exists cannot take the build-failure route, because a
manifest with no runtime may not enter `Building`; the events were refused one
at a time and the run ended in a state that offers no way to start again. Fixing
the key changed nothing: the only way out was to delete the application and
describe what you wanted a second time. A provider failure is now what it
actually is — a blocker to resolve — and no failed run may end anywhere it
cannot be retried from.

**Still not proven, and honest about it:** the repair round has never run
against a real model. Every failure so far has been the mock's. And none of this
is in CI, because none of it can be: it needs a daemon and a credential, which
is the same position the desktop window was in before somebody filmed it, and
the release workflow before somebody ran it.

**Three more things the same afternoon found; two are fixed.** A granted
directory has two names — the one on this machine, which is the one somebody
granted, and the one inside the sandbox under `/mnt`, which is the only one the
application can open — and only the first was ever printed. It reads as an
instruction, and following it fails on a file the application cannot possibly
see, which is exactly how the first two runs by hand went. Both names are
reported now, with the prefix to type in front of you. And `ephemeral logs`
withheld an application's own output once it had crashed, because output was
fetched only for states that still hold a container: the traceback explaining a
crash was the one thing not shown, while an application that had exited cleanly
and had nothing to explain handed its output over. A container outlives the run
that ended it, so the output is asked for whenever there might be one.

The third is a lifecycle question and is deliberately left alone: an application
that is `Ready` cannot be regenerated at all. `Ready` offers no event leading
back to planning, so `generate` answers "stop it first" about something that is
not running. The transition table withholds that route on purpose — a ready
application is running code somebody approved — so changing it is a decision
rather than a fix, and it is written down here instead of quietly made.

**A local model is the only answer to "the intent leaves the machine".** Every
other provider sends what somebody asked for to a company, and no amount of
policy changes that — which is why the threat model has always said the
mitigation is choice rather than prevention. `--provider local` is that choice:
Ollama, llama.cpp, LM Studio or vLLM on the loopback interface, generating
without the request leaving the machine.

It came almost free, because a local model server speaks OpenAI's chat
completions API — they all do — so the same wire format serves a hosted
alternative to Anthropic and a model on the user's own laptop. What is not free
is the promise. `local` is a destination, not a format, so the provider checks
one before every request: an endpoint that is not loopback is refused by name,
and the URLs built to look loopback and resolve elsewhere — the address as
userinfo, as a subdomain, as a label — are refused with it.

Two things are said out loud rather than implied. A model small enough to run on
a laptop fails at single-shot structured output more often than a hosted one,
and the honest description of `--provider local` includes that. And a local
model is not a *more trustworthy* model: its output is validated identically,
because privacy is not integrity.

**Rolling back was impossible, not merely unimplemented.** ADR-0011 made a
version immutable and content-addressed, and the manifest recorded the digest —
but nothing kept the bytes, and one `source/` directory per application was
overwritten by every generation. The history could say what an application had
been and could not put it back. Versions now have their own store, and the
domain model can return to one, and `ephemeral rollback` offers it.

Two things it carries through, both tested by breaking them first: the built
image is cleared, because running the newer image under the older version's
name would have the application report one identity and execute another; and a
rollback that *widens* — returning to a version the newer one had stopped
needing a capability for — withdraws the grants it would otherwise inherit,
because an approval given for different code is not an approval for this one.

**Rolling back had never been finished, and putting it in the window is what
showed it.** The terminal's version sequenced the steps itself, so the window
would have been a second copy of them — and a rollback is exactly the operation
whose steps must not come apart, since the source on disk goes back *before* the
grants the older version must not inherit are withdrawn. Moving it into
[`ephemeral-api`](../crates/ephemeral-api) meant writing it as one function with
tests that drive it through a real store, and those tests failed immediately on
something no test had reached before: **`ephemeral rollback` could not save.**

`revert_to` clears the built image, deliberately — running the newer build under
an older version's name would have the application report one thing and execute
another. The manifest's own validation then refused the result, because a
containerised runtime with no image "has nothing to run". Both rules were right;
they had simply never met, since the domain test asserted on the manifest in
memory and the storage test never rolled anything back. The invariant now asks
its question of a state that could actually be started: an application claiming
to be ready with nothing to run is still refused — and cannot even transition
into ready, which is asserted separately — while one that has just been rolled
back is in the ordinary condition of having source and no build.

The second thing the same test found: the advice `rollback` prints —
*generate again to rebuild* — was impossible to follow. Generation checked for
one lifecycle event, `Plan`, which only an application that has never been
generated can raise. Everything a rolled-back or failed application is in offers
`Retry` instead, so the answer was "cannot regenerate: it is blocked". It now
starts from whichever of the two the application can actually raise.

What the window adds beyond the button is the part a person needs before
clicking it: which versions can be returned to at all. A version can be recorded
in the history with its source swept away by retention, and "recorded" is not
"restorable" — so the view carries both facts, and a version whose source is
gone is drawn as an absence rather than offered as a button that cannot work.
Rolling back is asked twice, because clearing a build and taking permissions
back are not undone by clicking again.

### Phase 3 — Permissions

Enforcement, not just modelling: meta-permissions wired to real operations, app
permissions enforced at the runtime boundary, the permission UI, the audit log
in the loop, and the sandbox.

| | |
|---|---|
| Ephemeral's own authority asked for before it drives a container runtime | ✅ |
| …before it reaches a hosted model, and before it uses a credential | ✅ |
| An application's capability requiring **both** halves — its grant and Ephemeral's ([ADR-0003](architecture/decisions/0003-two-tier-permission-model.md)) | ✅ |
| Revoking a meta-permission emptying every sandbox at once | ✅ |
| Revocation reaching what is already running, rather than only the next start | ✅ |
| Views reporting what an application can *use*, so a page and a sandbox cannot disagree | ✅ |
| Inert capabilities shown as inert, in the terminal and in the window | ✅ |
| `ephemeral doctor` reporting what Ephemeral may do and what it is missing | ✅ |
| [Every promise mapped to the code that enforces it](security/enforcement.md) | ✅ |

**Done when:** every test in `tests/security.rs` is backed by an enforcement
point rather than by the domain model alone — which is
[this table](security/enforcement.md), and it has no blanks.

**The model was complete and nothing consulted it.** `PermissionLedger::check_app`
existed, applied both halves of ADR-0003 correctly, and carried a doc comment
naming itself "the check enforcement points should use". Every enforcement point
used something else. The sandbox was built by filtering an application's own
grants, so Ephemeral's authority was a column in a ledger that changed nothing;
Docker was driven without asking whether Ephemeral may drive Docker; a model
provider was called without asking whether Ephemeral may reach the network. A
permission system nothing consults is a description of a permission system, and
the tests all passed because they asked the model rather than the product.

What that cost is visible in the diff: three existing tests had to change,
because each of them asserted that a grant reached the sandbox with only half
the model satisfied. That is what an unenforced rule looks like from the inside.

**Default deny now applies to Ephemeral too**, which is a real cost and worth
stating: a new installation cannot build, run or generate until somebody allows
it to. Every refusal names the missing authority and the command that grants it,
`ephemeral doctor` lists what is missing before anybody hits it, and
`ephemeral permissions <app>` shows both halves on one page — because "nothing
happened" with no explanation is how a security model teaches people to turn it
off.

**Revocation reaches the present, not just the future.** A sandbox is built once,
at start, so revoking a grant an application is *currently* running with used to
change what the next container would get and nothing about the one holding the
mount. Anything running on what was just taken back is stopped, and revoking
Ephemeral's own authority stops everything holding a container.

**Still not enforced, and written down rather than implied:** a container escape
defeats all of it; disk ceilings for an application's own storage are declared
and unenforced, because `--storage-opt` needs a backing filesystem Ephemeral
cannot assume; a persuasive lie in a permission request is detected by nothing;
and the window can show that Ephemeral lacks an authority but cannot grant it —
that is the most powerful consent in the product and it currently belongs to the
terminal. The [enforcement map](security/enforcement.md) ends with that list.

### Phase 4 — Desktop

A Tauri window over [`ephemeral-api`](../crates/ephemeral-api), so the window
and the terminal show the same views worded identically.

| | |
|---|---|
| `ephemeral-api` — the versioned service layer both clients consume | ✅ |
| The window, its commands, and a frontend with no build step | ✅ |
| Rendering tested in headless Chromium, including that untrusted names cannot become markup | ✅ |
| Deciding permissions from the window, under the same rules as the terminal | ✅ |
| Asking for an application from the window, without opening a terminal | ✅ |
| Returning an application to an earlier version from the window | ✅ |
| Running and generating from the window | ✅ |
| `ephemeral-engine` — generating and running, as one implementation both clients call | ✅ |
| Reading what an application has been and what it printed, without a terminal | ✅ |
| Archiving, restoring, deleting and purging from the window | ✅ |
| Ephemeral's own authority, granted and taken back from the window | ✅ |
| What this machine can and cannot do, and the security record, on screen | ✅ |
| The record reconciled against the containers before it is drawn | ✅ |
| Suspending and picking an application back up, as the terminal can | ✅ |
| Buttons taken from the lifecycle's own answer rather than inferred | ✅ |
| The real window run and filmed under WebKitGTK, not only in Chromium | ✅ |

**Done when:** somebody can do everything the CLI does without opening a
terminal.

**What was actually in the way was not the window.** Generating and running
lived inside the CLI crate, so a window could not call them: it could either
have its own copy of "plan, write, build, repair, record" — the second, subtly
different Ephemeral that the service layer exists to prevent — or it could do
nothing. `ephemeral-api` was not the place to put them either, because it holds
no I/O on purpose: it compiles for a phone, where there is no daemon and no
subprocess.

So the split is three ways now. The core is the domain; `ephemeral-api` is what
every client can do; `ephemeral-engine` is what a client with a machine
underneath it can do. Three thousand lines moved, and the CLI kept what a CLI is
for — resolving what somebody typed, and drawing the answer. Its behaviour is
unchanged, checked by running it rather than by assuming: create, generate
against real Docker, grant both halves, run, read the output.

**Progress is the application's own lifecycle, not a bar.** Generation takes
minutes, so the window starts it on a thread and re-reads the application while
it runs — planning, writing the app, building, testing — because those states
are saved to disk as they happen. A progress bar would have been a number
nothing measures. Leaving the page does not stop it, and coming back finds
either a running application or a finished one.

**Filming the real window found the one bug the tests could not.** Everything
above was checked in headless Chromium, which draws the page perfectly and knows
nothing about the machine underneath it. Run the actual binary against a virtual
X server ([`tests/film-window.sh`](../apps/desktop/tests/film-window.sh)) and
the first frame said "Running" about a container that had exited long before.
Nothing was wrong with the rendering: the record itself was stale, and the
terminal only avoids this because `ephemeral watch` exists and somebody types
it. A window is already redrawing, so it now reconciles first and draws second —
which is what a person opening it is trying to find out. The lesson is the older
one restated: a surface nobody has run is a surface with problems no test finds,
and that stays true right up until somebody runs it.

**Listing the terminal's commands against the window's found two more gaps.**
Pause and resume were simply absent — the terminal has had both since Phase 1,
and "everything the terminal does" is the kind of claim that survives until
somebody writes the two lists side by side. The other was subtler: the window
worked out its own buttons from a few booleans on the view, and one of those
booleans meant "must know what it runs on" rather than "is running", so a built
application that had never been started was offered Stop and could not be
archived. The state machine has always been able to answer this exactly —
`available_events(Actor::User)`, whose own documentation says it exists so that
"a user is never shown an action that would be refused" — and nothing was
asking it. The service layer carries that answer now, as `can`, and both clients
draw from it.

**Two things stayed with the terminal, deliberately.** Granting Ephemeral a
*scoped* authority (`read:~/Downloads/**`) means choosing a region of the
filesystem, and a window that composed a path from a text field would be a
window that can grant Ephemeral something nobody typed — so it offers the three
unscoped ones, shows everything held, and can take any of it back. And
publishing and installing need a folder picker, which needs a Tauri plugin and
therefore a build step the window does not have; that is
[Phase 7](#phase-7--sharing) work, and it is listed there rather than pretended
about here.

Two smaller ones, for completeness. `ephemeral states` prints the whole state
machine as a reference; the window shows an application's own history and what
its current state means, which is the half a person on that page is asking
about. And `ephemeral grant` can give an application a capability it never
requested, while the window only ever answers what was asked — that is the
consent model working as intended, not a missing button.

### Phase 5 — Cross-platform

| | |
|---|---|
| Tests on Linux, macOS and Windows on every commit | ✅ |
| CLI archives for six targets, including ARM Linux and ARM Windows | ✅ |
| Desktop installers: `.deb`, `.rpm`, AppImage, a universal `.dmg`, NSIS and MSI | ✅ |
| Checksums over the published artifacts, and draft releases | ✅ |
| Signing and notarisation, which need credentials this repository must never hold | |
| A C ABI for mobile, with the host supplying its own HTTPS transport ([ADR-0017](architecture/decisions/0017-mobile-generates-through-a-host-transport.md)) | ✅ |
| iOS and Android libraries built and published, and checked against the header on every commit | ✅ |
| The Kotlin shell: an Android application, and an `.apk` in the release | ✅ |
| The Kotlin shell compiles on every commit, not only when a release is cut | ✅ |
| The phone's screens photographed on an emulator, so somebody can look at them | ✅ |
| Signing wired to repository secrets, inert until an identity exists | ✅ |
| The JNI bridge driven from a real JVM on every commit, callback included | ✅ |
| The Swift shell, type-checked against the iOS SDK on every commit | ✅ |
| An iOS application somebody can install, which needs an Xcode project and an identity | |
| A run on a physical device, which needs a device cloud account or a phone on a wire | |
| Building and running on mobile, which needs the control plane in [ADR-0007](architecture/decisions/0007-mobile-control-plane.md) | |

**Done when:** somebody can download and run Ephemeral on their own machine
without building it.

The first version of this row said the release workflow was done. It had never
been run: releases are cut from tags, no tag had been cut, and the workflow
called CI as a reusable workflow that had never declared itself reusable, built
binaries where it claimed to build installers, and depended on two icon formats
that did not exist. Running it is what made any of that visible — the same
lesson as filming the window, in a different medium.

**Mobile was blocked by a transport, not by phones.** This row read "mobile,
which needs the control plane" for as long as it existed, on the strength of
ADR-0007 having said generation and execution were one thing. They are not.
What actually stopped a phone generating was ADR-0016's `curl` subprocess —
iOS does not let an application spawn a process — and nothing in the repository
said so, because nothing had tried. Making transport a trait unblocked it in an
afternoon; the seam then paid for itself immediately, since the provider's
request building and error mapping had been untestable by construction and are
now driven by fake transports in CI.

**The Swift is code now, not a snippet.** For as long as this row existed the
only Swift in the repository was an example in `docs/mobile.md` — which is to
say, code nobody had ever put through a compiler. `apps/ios` is the shell: the
engine wrapper, a URLSession transport, the Keychain, and every screen, in the
same palette and the same words as the window. CI type-checks all of it against
the real iOS SDK with the real C header on every commit.

What that is not is an application anybody can install. That needs an Xcode
project — these are sources, not a target — and an identity to sign with, and
nothing has run it. It is in the position the desktop window was in before
somebody filmed it, and `apps/ios/README.md` says so on its own first screen
rather than leaving it to be discovered.

**Android has an application; iOS has a library and now a shell.** The Kotlin shell exists, in
[`apps/android`](../apps/android), and ships as an APK. It declares no
dependencies at all — the engine is the only thing it needs, and HTTPS, JSON,
the keystore and the widgets are already in the platform — which is why it
carries no AndroidX and uses plain views.

The bridge between Kotlin and the C ABI is JNI, and JNI resolves by name at run
time: a symbol or signature that drifts compiles perfectly and fails on a phone.
So `crates/ephemeral-android` is built for every target rather than only for
Android, and `tests/jni.sh` loads it into an ordinary `java` process and drives
it — including the callback into Java that generation depends on, a transport
that refuses, and a transport that throws. Restricting that crate to Android
would have made the one piece of code here that is easy to get wrong the one
piece nothing could test without a phone.

What that does **not** cover is the application's own screens. Nobody has run
this on a device; there is no KVM in the container it was written in, so there
was no emulator either. It is in exactly the position the desktop window was in
before it was filmed, and it should be read that way.

**A real device is reachable; it needs an account, not an invention.** "No
machine in CI is a phone" was the reason this row gave for years, and it stopped
being the whole truth the moment device clouds existed. The routes, honestly:

- **A phone on a wire is the cheapest and the best.**
  `apps/android/tests/photograph.py` drives whatever `adb` is talking to, so it
  already works against a real handset plugged into a laptop — no account, no
  secrets, no per-minute billing, and the frames come from the actual hardware.
  This is the one to do first, and it needs nobody's permission.
- **[Firebase Test Lab](https://firebase.google.com/docs/test-lab/usage-quotas-pricing)**
  runs on real Android hardware and its free tier covers five physical-device
  runs a day. Its Robo crawler walks an application by itself and returns
  screenshots, video and crash logs, which is close to what this repository
  already means by "looking". **The workflow for it is written** —
  `.github/workflows/device.yml`, dispatch-only — and inert until two
  repository secrets exist; `apps/android/README.md` has the twenty minutes of
  setup, and the workflow says which secret is missing rather than failing
  obscurely. Wired now for the same reason the signing variables were: doing it
  later means discovering the wiring is wrong at the moment somebody wanted an
  answer.
- **[AWS Device Farm](https://docs.aws.amazon.com/devicefarm/latest/developerguide/apps.html)**
  runs both platforms on real hardware, and is the interesting one for iOS: it
  re-signs an uploaded application with its own certificate and a wildcard
  profile, so it needs no Apple Developer account and no device UDIDs. It does
  need an `.ipa` built for a device rather than a simulator — which means the
  Xcode project is the blocker there, not the identity.
- **BrowserStack, Sauce Labs and LambdaTest** all rent real devices and all have
  open-source plans. Same shape: credentials in secrets.

Every one of them is a credential this repository does not hold and a third
party in a pipeline that also signs releases — the same judgement as the
signing identity, and the same answer: wire it when there is an account to wire,
say plainly until then that nothing has run on a phone.

**Signing is the remaining gap, and it is not a small one.** Until these builds
are signed, macOS refuses to open the application and Windows warns before
running the installer. Both are correct to. It needs an Apple Developer account
and a Windows code-signing certificate — paid identities belonging to a person
or an organisation, not to a repository — after which the keys live in
repository secrets and never in the tree.

The wiring for it is in place and inert. The bundle step reads the six Apple
variables Tauri signs and notarises from, and skips both when they are empty,
so the day a certificate exists is a day of adding secrets rather than a day of
editing a release workflow under time pressure — which is exactly the situation
in which the rest of this workflow was found to be wrong. A step after it asks
the platform whether the build actually came out signed and warns when it did
not, because a release whose signing quietly did not happen looks identical to
one where it did until somebody's machine refuses to open it. There is
deliberately no self-signed fallback: an unsigned build that says so is more
honest than a signed one nobody can trace.

**The phone's screens have been photographed, and it went exactly like the
window.** `.github/workflows/screens.yml` builds the engine and the
application, boots an emulator on a runner that has KVM, and runs
`apps/android/tests/photograph.py`, which taps controls found by resource id
rather than by coordinate. An emulator is not a device and this does not close
the row below; what it closes is that nothing had ever drawn these screens at
all.

Getting there took four attempts and none of the failures were Android's: a
`yes |` pipeline killed by SIGPIPE under `pipefail`, runs cancelling each other
through their own concurrency group, `avdmanager` writing the AVD where
`emulator` does not look, and `/dev/kvm` existing while being unwritable by the
job — a check that asked whether the file was there had reported everything was
fine. Every one of them was read out of a log rather than guessed at, which is
the only reason there were four rather than one a week.

Then the photographs found four more, in the application itself. The best of
them: **the list crashed the moment it contained one application.**
`ArrayAdapter` treats a bare layout as a `TextView` unless you name the view id
inside it, so the card the new design introduced threw the first time a row was
drawn. It compiled, CI was green, and every screenshot until that one had been
of an empty list — which never draws a row. The others are in
[development.md](development.md#the-phone-applications).

It runs outside the required checks, because a screenshot is something a person
reviews rather than something that passes.

### Phase 6 — Hardening

The [threat model](../SECURITY.md#threat-model), security testing, supply-chain
work, performance, recovery, installers and release automation.

[The threat model](security/threat-model.md) is written. Every mitigation it
names either exists with a test, or is listed as a gap — including the ones that
are uncomfortable: a container escape defeats the sandbox, a user who approves
everything is barely protected, disk ceilings are declared and unenforced, and
a persuasive lie in a permission request is not detected by anything.

**Done when:** the remaining gaps are either closed or accepted deliberately,
and release signing and SBOMs exist — which needs there to be releases.

### Phase 7 — Sharing

Giving an application to somebody else, publishing it, and — separately — letting
several people use one running instance. Designed in [sharing.md](sharing.md);
decided in [ADR-0011](architecture/decisions/0011-immutable-content-addressed-versions.md),
[ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md) and
[ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md).

Sits on top of nearly everything else: it needs a runtime to build a received
recipe, generation to produce versions, sandboxing to make accepting a
stranger's application reasonable, and the threat model — shared sessions are
the largest expansion of it so far.

One part is **not** deferrable to Phase 7: immutable, content-addressed versions
belong with Phase 2, because that is when versions start being produced and
identity cannot be retrofitted onto history that was never recorded.

Done so far:

| | |
|---|---|
| `ephemeral publish` — a package that is an ordinary, reviewable directory | ✅ |
| `ephemeral install` — review first, accept second, with no permissions | ✅ |
| Grants and everything local kept out of a package | ✅ |

Still to do: updating an installed application, with the permission delta put
to the recipient before it applies; optional signing, scoped honestly to
authorship; and shared sessions, which need the relay design in ADR-0013.

**Done when:** an application can be published to a git host, installed by
somebody else, and run under permissions *they* granted — with an update that
wants more than the version they approved refused until they decide.

**Decided:** a group operates its own relay, and there is no other kind —
Ephemeral does not run one and there is no third-party opt-in. That costs reach
deliberately: a group where nobody will keep a device on does not get a shared
session. [ADR-0013](architecture/decisions/0013-how-several-people-share-an-application.md)
is accepted.

## What cannot be built from here

Two remaining pieces need something this repository does not have, and saying so
is better than shipping a plausible-looking version of either.

**A provider's transport cannot be tested here**, because doing so needs a
credential and a live call, which
[ADR-0008](architecture/decisions/0008-agent-provider-abstraction.md) forbids CI
from making. The provider crates are built so that this costs as little as
possible: prompts, request bodies, response parsing, capability translation and
error mapping are pure and tested, and the untested part is one module, shared
by all three, that hands a string to `curl`. CI guards that split rather than
trusting it.

`--provider local` narrows the gap without closing it. Nothing in CI runs a
model server either, so what has never been exercised here is the same for
`local` as for the hosted providers: the request actually going out and a real
model's reply coming back.

**The desktop window has been looked at now, on one platform.** It was run
against a virtual X server and filmed — the real binary, WebKitGTK, the window
frame and all — and that immediately found something no test had: it said
"Running" about a container that had exited long before, because nothing
reconciled the record before drawing it. Fixed, and the fix is tested. Two gaps
remain and neither closes from here: nobody has opened it on macOS or Windows,
which render through WebView2 and WKWebView rather than WebKitGTK, and a
recording is not a person — nothing here has been *used*, only watched.

**Signing and notarisation** need credentials this repository must never hold.
The release workflow produces checksums, which say a file is intact and
deliberately do not claim who made it.

## Things deliberately not being built yet

Not everything absent is an oversight. These are decided-against-for-now, with
the reasoning recorded:

- **App-to-app composition.** Reserved in [ARCHITECTURE.md](../ARCHITECTURE.md#12-app-to-app-composition)
  as explicit capability contracts. Not in the MVP.
- **Plugins.** The seams exist; the plugin system does not.
- **Cloud sync.** Desktop is local-first, and stays useful without a server.
- **A WebAssembly runtime.** The most likely future addition to the runtime
  trait, and the reason the trait exists — but today's ecosystem cannot run the
  general case.
- **A central Ephemeral registry.** Git hosting already distributes recipes, and
  a curated registry implies a safety judgement the project is not in a position
  to make ([ADR-0012](architecture/decisions/0012-sharing-distributes-recipes.md)).
