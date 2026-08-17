> **This is a pre-release.** Ephemeral is mid-Phase 2 of seven, nothing here is
> signed by a known identity, and the Android application has never been run on
> a physical device. [The roadmap](https://github.com/JGalego/Ephemeral/blob/main/docs/roadmap.md)
> says which phases are finished and which are not, and
> [the threat model](https://github.com/JGalego/Ephemeral/blob/main/docs/security/threat-model.md)
> says what Ephemeral does not defend against. Both are worth reading before you
> point this at anything you care about.

## Installing

| You want | Download |
|---|---|
| The window, on Linux | `.deb` (Debian, Ubuntu), `.rpm` (Fedora, RHEL), or `.AppImage` (anything) |
| The window, on macOS | `.dmg` — one file, both Apple and Intel machines |
| The window, on Windows | `-setup.exe` to double-click, or `.msi` to deploy |
| The command line | `ephemeral-<version>-<target>.tar.gz`, or `.zip` on Windows |
| Ephemeral on an Android phone | `ephemeral-<version>.apk` |
| To build a phone app on it | `Ephemeral-<version>-ios.xcframework.zip`, or `ephemeral-<version>-android.tar.gz` |

Full instructions, including how to put `ephemeral` on your `PATH`, are in
[docs/install.md](https://github.com/JGalego/Ephemeral/blob/main/docs/install.md).

## On a phone

**Android has an application** — `ephemeral-<version>.apk`, Android 8.0 and
later. **iOS does not yet**: its download is the engine, a static library and a
C header, for somebody writing the Swift shell around it. The
`ephemeral-<version>-android.tar.gz` is the same thing for anyone building
their own Android app rather than installing this one.

What Ephemeral does on a phone is narrower than on a desktop, on purpose. It
**generates**: a sentence becomes an application, its source written to the
phone, using the app's own HTTPS client and a credential from Keychain or
Keystore. It deliberately does **not** build or run that application — that
needs a sandbox no phone has, and running generated code outside one is the
thing Ephemeral exists to prevent. The app says so on its first screen. The
reasoning is in
[ADR-0017](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md).

Nobody has run the Android app on a physical device. Its bridge to the engine
is tested on every commit; its screens have been seen by no one.

## These builds are not signed

Nothing here carries a code-signing signature, and the honest consequence is
that your operating system will treat it as software from nobody:

- **macOS** will refuse to open it — "Ephemeral is damaged" or "cannot be
  opened because the developer cannot be verified". It is not damaged; it is
  unsigned and un-notarised. Right-click the app and choose **Open**, or run
  `xattr -dr com.apple.quarantine /Applications/Ephemeral.app`.
- **Windows** SmartScreen will warn you before running the installer. **More
  info** → **Run anyway**.
- **Linux** does not check, so it will simply install.
- **Android** refuses to install anything unsigned at all, so the APK *is*
  signed — with a key generated for this release and then destroyed, because a
  signing key is a credential and does not live in the repository. Your phone
  will still ask you to permit installing unknown apps, and **this release will
  not install over an earlier one**: Android rejects an upgrade whose signature
  changed, and every release's differs. Uninstall the old one first, which
  takes its workspace with it.

Do not wave those warnings away because a README told you to. They are the
operating system correctly reporting that it cannot tell who built this file.
The checksums below let you verify the file is *intact*; they cannot tell you
who made it. Those are different claims and Ephemeral does not conflate them.

Signing and notarisation are tracked as a gap in
[docs/roadmap.md](https://github.com/JGalego/Ephemeral/blob/main/docs/roadmap.md).

## Verifying what you downloaded

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

On macOS use `shasum -a 256 -c`, and on Windows
`Get-FileHash <file> -Algorithm SHA256`.

`SHA256SUMS.txt` covers every file in this release and is generated from the
artifacts themselves at publish time.
