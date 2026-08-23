# Ephemeral for iOS

The same engine the desktop runs, on a phone. Not a second implementation of
anything: the lifecycle machine, the permission ledger and the audit record are
the Rust in `crates/`, reached through the C ABI in `crates/ephemeral-ffi`.

## What it does, and what it deliberately does not

**It records and it generates.** You describe what you want, Ephemeral plans it,
asks a model for the source, and writes that source to the phone. The
permissions the generated application asks for are recorded, and you answer
them.

**It does not build or run what it generated.** That needs a sandbox, and a
phone has none a third-party application may use. Running generated code outside
one is the specific thing Ephemeral exists to prevent, so it is not done on a
phone as a convenience. The reasoning is in [ADR-0007] and [ADR-0017].

This is said on the first screen, not buried here. Somebody who taps *Generate*
and waits for a build that will never happen has been misled by the application,
not by the engine.

A machine that can build finishes the job — the same workspace, on a desktop.

## Layout

| Path | What it is |
|---|---|
| `Sources/Ephemeral/EphemeralApp.swift` | The application, which is four lines and one decision |
| `Sources/Ephemeral/Views.swift` | Every screen |
| `Sources/Ephemeral/Engine.swift` | The only thing allowed to call the engine |
| `Sources/Ephemeral/Transport.swift` | The HTTPS this application performs on the engine's behalf |
| `Sources/Ephemeral/Credential.swift` | The model key, in the Keychain |
| `Sources/Ephemeral/Model.swift` | Which service generates, and how it is configured |
| `Sources/Ephemeral/Palette.swift` | Generated. Do not edit — see [docs/design.md](../../docs/design.md) |
| `typecheck.sh` | Puts all of the above through a compiler |

## What state this is in

**It type-checks, and nothing has run it.** `typecheck.sh` compiles every source
file against the real iOS SDK with the real C header, and CI does that on every
commit — which is more than the snippet in `docs/mobile.md` ever got, and less
than "it works". Read it the way the desktop window was read before somebody
filmed it.

What is missing before there is an application anybody can install:

- **An Xcode project.** These are sources, not a target. Producing an `.ipa`
  needs a project (or a generator for one) and a bundle identifier.
- **An identity to sign with.** Apple will not install an unsigned application
  on a device, and an Apple Developer account is a paid identity belonging to a
  person, not to a repository. The release workflow already reads the signing
  variables from repository secrets and skips signing when they are empty.
- **A device.** Nobody has run this on one. There is no emulator in the
  container it was written in, and an iOS simulator needs a Mac.

## Building it, when there is a project

The engine, as a static library for the simulator and the device:

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build -p ephemeral-ffi --release --target aarch64-apple-ios
cargo build -p ephemeral-ffi --release --target aarch64-apple-ios-sim
```

`crates/ephemeral-ffi/tests/device-targets.sh` builds exactly those and checks
the archive's symbols against the header, on every commit, without Xcode — the
one part of the trip to a device that can be checked from anywhere.

Assemble them into an XCFramework alongside `include/ephemeral.h` and
`include/module.modulemap`, add it to the target, and `import Ephemeral`
resolves.

## The palette

`Palette.swift` is generated from `crates/ephemeral-design`, which is where the
desktop window's colours and the Android application's colours come from too.
Do not edit it; run `cargo run -p ephemeral-design`. Every colour in it is
checked for contrast by that crate's tests, and a risk level nobody can read is
a permission prompt that does not work.

[ADR-0007]: ../../docs/architecture/decisions/0007-mobile-control-plane.md
[ADR-0017]: ../../docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md
