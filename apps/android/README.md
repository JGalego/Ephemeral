# Ephemeral for Android

An application you can install on a phone, running the same engine the desktop
runs. Not a second implementation of anything: the lifecycle machine, the
permission ledger and the audit record are the Rust in `crates/`, reached
through the C ABI in `crates/ephemeral-ffi`.

## What it does, and what it deliberately does not

**It records and it generates.** You describe what you want, Ephemeral plans it,
asks a model for the source, and writes that source to the phone. The
permissions the generated application asks for are recorded, and you answer
them.

**It does not build or run what it generated.** That needs a sandbox, and a
phone has none that a third-party app may use. Running generated code outside
one is the specific thing Ephemeral exists to prevent, so it is not done on a
phone as a convenience. The reasoning is in [ADR-0007] and [ADR-0017].

This is said on the first screen of the app, not buried here. Somebody who taps
*Generate* and waits for a build that will never happen has been misled by the
app, not by the engine.

A machine that can build finishes the job — the same workspace, on a desktop.

## Layout

| Path | What it is |
|---|---|
| `build-native.sh` | Cross-compiles the engine into `app/src/main/jniLibs/` |
| `app/src/main/java/…/Native.kt` | The engine's functions, and nothing else |
| `app/src/main/java/…/Engine.kt` | The only thing allowed to call them |
| `app/src/main/java/…/Transport.kt` | The HTTPS this app performs on the engine's behalf |
| `app/src/main/java/…/Credential.kt` | The model key, sealed by the Android keystore |
| `crates/ephemeral-android` | The JNI bridge, in Rust, at the repository root |

## Looking at it

`tests/photograph.py` drives whatever device `adb` is talking to — a plugged-in
phone as readily as an emulator — taps through the application and writes a
numbered frame per step. It asserts nothing on purpose: you have to look.

```bash
adb devices                       # a phone on a wire, or a running emulator
apps/android/tests/photograph.py  # frames into ./screens
```

`.github/workflows/screens.yml` runs exactly that against an emulator on a
runner with KVM. An emulator is not a device; [the roadmap](../../docs/roadmap.md#phase-5--cross-platform)
lists what running on real hardware would take.

## Building

You need the Android SDK (platform 34, build-tools 34), an NDK, a JDK 17 or
newer, and the four Android Rust targets:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi \
                  x86_64-linux-android i686-linux-android
sdkmanager "platforms;android-34" "build-tools;34.0.0" "ndk;26.3.11579264"
```

Then, from this directory:

```bash
ANDROID_NDK_HOME=$ANDROID_HOME/ndk/26.3.11579264 ./build-native.sh
gradle assembleDebug
```

The APK lands in `app/build/outputs/apk/debug/`.

Gradle does not drive the Rust build. Cross-compiling from a Gradle task hides
the failure inside a build system that cannot explain it, and it makes the
command different here and in CI. `build-native.sh` is the command in both.

## Signing, and why the release APK is awkward

Android will not install an unsigned APK at all — unlike macOS and Windows,
where an unsigned build merely warns you. So a release build has to be signed
with *something*, and a signing key is a credential, which [SECURITY.md] says
does not live in this repository.

Until a real key lives in repository secrets, **CI generates a throwaway key for
each release**. One consequence is worth knowing before you install:

> Android refuses to upgrade an installed application when the new signature
> differs from the old one. A new Ephemeral release will not install over an
> older one — uninstall first. Uninstalling takes its workspace with it.

That is the Android equivalent of the unsigned `.dmg` warning, and it goes away
the same way: with a real signing identity. See [docs/install.md].

To sign a local build with your own key:

```bash
export EPHEMERAL_ANDROID_KEYSTORE=/path/to/keystore.jks
export EPHEMERAL_ANDROID_KEYSTORE_PASSWORD=…
export EPHEMERAL_ANDROID_KEY_ALIAS=…
export EPHEMERAL_ANDROID_KEY_PASSWORD=…
gradle assembleRelease
```

Without those, `assembleRelease` produces an unsigned APK you can inspect but
not install.

## The model key

Entered in the app, sealed with an AES key held in the Android keystore, and
handed to the engine in memory for the duration of a call. It is never written
to Ephemeral's files, never put in the audit log, and never read from an
environment variable — that is a desktop convention and a phone has no
equivalent.

The engine has no way to forget a credential mid-session, so *Forget* ends the
session; the next call opens a fresh one without it.

## No dependencies

The app declares none. Ephemeral's engine is the only thing it needs, and
everything else it uses — HTTPS, JSON, the keystore, the widgets — is in the
platform. That keeps the APK small, the build hermetic, and the list of third
parties with code on your phone as short as it can honestly be.

It also means no AndroidX, which is why the code uses `android.app.Activity` and
plain views rather than what a new Android project would start with. That is a
deliberate trade, not an old template.

[ADR-0007]: ../../docs/architecture/decisions/0007-mobile-control-plane.md
[ADR-0017]: ../../docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md
[SECURITY.md]: ../../SECURITY.md
[docs/install.md]: ../../docs/install.md
