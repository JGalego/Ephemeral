# ADR-0018: Android gets an application, and its JNI bridge is testable without a phone

- **Status:** accepted
- **Date:** 2026-08-17
- **Deciders:** Ephemeral maintainers
- **Phase:** 5 — Cross-platform

## Context

[ADR-0017](0017-mobile-generates-through-a-host-transport.md) made generating on
a phone possible: the C ABI in `ephemeral-ffi`, with the host supplying HTTPS
through two function pointers. What it produced was a library. Nobody could
install Ephemeral on a phone, and the honest summary of "Android support" was a
`.a` file and a header.

Three things stood between that and an application somebody could install.

**Kotlin cannot produce a C function pointer.** `ephemeral_open` needs two of
them. Every other call could be reached with a thin foreign-function layer; the
transport callback could not, and the transport callback is the one that makes
generation work at all.

**Android will not install an unsigned APK.** Unlike macOS and Windows, where an
unsigned build merely warns, Android refuses outright. So "ship it unsigned like
the desktop builds" was not an available option, while
[SECURITY.md](../../../SECURITY.md) says a signing key does not live in this
repository.

**A JNI symbol is resolved by name at run time.** A method whose name or
signature has drifted from the native side compiles perfectly, packages
perfectly, installs perfectly, and dies on the device. That is precisely the
failure mode the desktop window taught this project to distrust — code that
looks finished because nothing ever ran it.

## Decision

**An Android application, in `apps/android`, with a JNI bridge in
`crates/ephemeral-android`.**

The bridge is Rust, and it forwards to the same C ABI an iOS application would
call. It contains no domain logic and makes no decisions: the lifecycle machine,
the permission ledger and the audit record stay in `ephemeral-core`, reached the
same way from every platform. What it adds is the one thing Kotlin cannot
express — a C function pointer that calls a Java method.

**The bridge crate is built for every target, not only for Android.** JNI is the
same on a desktop JVM, so `crates/ephemeral-android/tests/jni.sh` loads the
library into an ordinary `java` process and drives it: create, list, inspect, an
unknown id, a generation whose transport refuses, and a generation whose
transport throws. It runs in `scripts/check` and in CI.

This is the part worth arguing about, because the obvious choice is the wrong
one. Gating the crate on `cfg(target_os = "android")` would keep `jni` out of a
desktop dependency tree — tidy, and it would have made the single piece of code
here that is easy to get wrong the single piece nothing could test without a
phone.

**The application declares no dependencies.** Not AndroidX, not a networking
library, not a JSON library. Ephemeral's engine is the only thing it needs;
HTTPS, JSON, the keystore and the widgets are in the platform. That is why the
code uses `android.app.Activity` and plain views rather than what a new Android
project would start with.

**Release APKs are signed with a key generated per release and destroyed with
the runner.** The key is a credential and does not go in the tree.

## Consequences

**Somebody can install Ephemeral on a phone.** That is the point.

**A new release will not install over an old one.** Android refuses an upgrade
whose signing certificate changed, and with a per-release key every one differs.
Uninstalling takes the workspace with it. This is stated in
[install.md](../../install.md) rather than discovered, and it ends when a real
signing identity exists — the same condition that ends the macOS and Windows
warnings.

**The `jni` crate is in the workspace's dependency tree on every platform.** A
real cost, accepted for a real test. `cargo deny` covers it like anything else.

**The bridge is tested; the screens are not.** `jni.sh` proves the boundary
holds, including the callback into Java. It proves nothing about what the
application looks like or whether its flow makes sense, and it was written in a
container with no KVM, so no emulator ran either. The Android app is in the
position the desktop window was in before it was filmed, and the roadmap says
so.

**Two C-ABI clients now exist, and only one is exercised in CI.** The JNI bridge
and the (still unwritten) Swift shell both depend on `ephemeral.h` being
truthful. When the Swift shell arrives it will need its own version of this, for
the same reason.

## Alternatives considered

**Kotlin calling the C ABI directly, through a foreign-function layer.** Does
not survive contact with `ephemeral_open`, which needs function pointers. The
JNI-callback path would have had to exist anyway, in a less examinable place.

**Putting the JNI entry points in `ephemeral-ffi` behind a feature.** Would have
mixed two calling conventions in the crate that defines the published boundary,
and made `ephemeral-ffi`'s dependency set depend on which platform was being
built. A separate crate keeps the C ABI the one boundary and the JNI layer an
adapter on top of it.

**Gradle driving the Rust cross-compilation.** Hides a toolchain failure inside
a build system that cannot explain it, and makes the command different locally
and in CI. `apps/android/build-native.sh` is the command in both.

**Waiting for a signing identity before shipping anything.** Would have left
Android with a library and no application for as long as an account took to
arrange, for a warning macOS and Windows users are already being asked to read
and understand.
