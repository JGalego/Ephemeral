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
runner with KVM. An emulator is not a device.

### On a real phone, over a wire

Nothing to set up. `photograph.py` drives whatever `adb` is talking to, so a
handset with USB debugging turned on works exactly like the emulator does — and
the frames come from real hardware, real fonts and a real GPU. This is the
cheapest and the best of the options here, and the only one that needs nobody's
account.

### On a real phone, in CI

`.github/workflows/device.yml` runs the application on a physical device in
[Firebase Test Lab](https://firebase.google.com/docs/test-lab). It is
dispatch-only: it spends somebody's quota and needs a credential this
repository does not hold, so it never runs unasked, and it says plainly which
secret is missing rather than failing obscurely.

Twenty minutes of setup, once, by somebody with a Google account:

1. **A Firebase project.** [console.firebase.google.com](https://console.firebase.google.com)
   → *Add project*. The Spark (free) plan is enough: it allows five
   physical-device runs a day, which is more than this needs. Note the project
   id — it is not the display name.
2. **Enable two APIs** in the Google Cloud console for that same project:
   *Cloud Testing API* (`testing.googleapis.com`) and *Cloud Tool Results API*
   (`toolresults.googleapis.com`). Test Lab is a Cloud service wearing a
   Firebase badge, and both halves have to be switched on.
3. **A service account** — Cloud console → *IAM & Admin* → *Service accounts* →
   *Create*. Name it `github-device-tests`. Give it the **Editor** role.

   Finding that role in the console is harder than it should be: typing
   "Editor" into the role picker returns dozens of per-service editors and not
   the plain one, which lives under the **Basic** category because Google
   discourages it. The unambiguous way is [Cloud
   Shell](https://console.cloud.google.com) — the `>_` button in the console's
   top bar, which needs nothing installed:

   ```bash
   gcloud projects add-iam-policy-binding YOUR_PROJECT_ID \
     --member="serviceAccount:github-device-tests@YOUR_PROJECT_ID.iam.gserviceaccount.com" \
     --role="roles/editor"
   ```

   Editor is broad, and it is what [Google's own Test Lab CI
   instructions](https://firebase.google.com/docs/test-lab/android/continuous)
   ask for. Narrower sets were tried here and each failed somewhere different
   and late: Test Lab uploads the application into a bucket it owns, writes
   results back, and reads tool-results resources, and no documented role
   covers exactly that. Three runs were spent discovering it one 403 at a time.

   The mitigation is not a narrower role, it is a narrower project. This
   project holds nothing but device-test results, is linked to nothing, and
   could be deleted tomorrow without losing anything. Keep it that way and
   Editor costs little; put anything else in it and Editor is too much.
4. **A JSON key** for that account — *Keys* → *Add key* → *JSON*. This is a
   long-lived credential. It belongs in repository secrets and nowhere else:
   not in the tree, not in a commit, not in a paste.
5. **Two repository secrets**, under *Settings → Secrets and variables →
   Actions*:
   - `FIREBASE_PROJECT_ID` — the project id from step 1
   - `FIREBASE_SERVICE_ACCOUNT` — the entire contents of the JSON file
   - `MODEL_API_KEY` — optional, and only for the **generate** input below
6. **Run it.** *Actions → Device → Run workflow*. Pick a device model; the
   default is a Pixel 8. If that model has been retired the run fails and
   prints every physical device that does exist, which is the list to choose
   from.

It uploads screenshots, a video and a crash log as an artifact. Robo explores
the application by itself; the one thing it cannot guess — what to type into
the box — is given to it as a directive using the same resource ids
`photograph.py` taps, so the two walkthroughs cannot drift into exercising
different screens.

### Generating on the phone

Every device run so far has stopped at the front door: with no key stored, the
app refuses to generate and says so, which is correct and is not very
interesting to photograph. Turning on the **generate** input puts the
`MODEL_API_KEY` secret into the app and lets Robo press Generate.

Three things are worth knowing before you do:

- **Which service is a setting**, and the run sets it. A rack phone is a fresh
  install every time, so whatever the workflow's `service`, `base_url` and
  `model` inputs say is the whole of what it knows — there is no "set it
  beforehand". `MODEL_API_KEY` has to match whichever service is named.
- **The run presses Check connection first**, so a wrong key or a retired model
  appears in the video as a sentence in the app rather than as a generation that
  quietly does nothing.
- **Generating is not building.** A phone plans the application and writes its
  source. It cannot build or run it: that needs a container runtime, which is
  why a phone is a control plane and not a second desktop ([ADR-0007]).
- **The key goes in on the command line.** Test Lab has no secret mechanism, so
  the only way to reach a phone in a rack is through the command that starts the
  run — which means the key is in gcloud's arguments here and in the run record
  Test Lab keeps. Everywhere else a credential travels on stdin for exactly that
  reason ([ADR-0016]). Use a key made for the run and revoked after it.

**A better credential, when it is worth the time.** A JSON key never expires
and works from anywhere it leaks to. [Workload Identity
Federation](https://github.com/google-github-actions/auth#preferred-direct-workload-identity-federation)
replaces it with a trust relationship between this repository and the project,
so there is no long-lived secret at all. It is more setup and it is the right
answer eventually.

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

## The model, and its key

*Model* in the menu chooses the service, its base URL and its model name, and
takes the key for it. One screen rather than three, because it is one decision:
a key belongs to a particular service, and a model name only means something
once a service is chosen.

**Check connection** asks that service what models it has. That single call
answers both questions somebody has before spending anything — whether the
credential works, and what may be named as a model — and it fills the model box
from what came back, so a name never has to be typed from memory. A failure
shows the service's own words.

The services are radio buttons rather than a spinner, and each carries a
resource id. That is not a style preference: `--robo-directives` addresses
controls by name and cannot operate a spinner at all, so with a spinner every
automated run on a real phone could only ever exercise whichever provider was
the default.

The list of services is read from the engine ([ADR-0020]) rather than written
into the app. An app ships on its own schedule; a list of providers hardcoded in
it is wrong the first time one is added.

The service and its settings are ordinary `SharedPreferences`. The key is not:
it is sealed with an AES key held in the Android keystore and handed to the
engine in memory for the duration of a call. It is never written to Ephemeral's
files, never put in the audit log, and never read from an environment
variable — that is a desktop convention and a phone has no equivalent.

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
[ADR-0020]: ../../docs/architecture/decisions/0020-the-host-chooses-the-provider.md
[ADR-0017]: ../../docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md
[SECURITY.md]: ../../SECURITY.md
[docs/install.md]: ../../docs/install.md
[ADR-0016]: ../../docs/architecture/decisions/0016-real-providers-live-in-their-own-crates.md
