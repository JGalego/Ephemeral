## Installing

| You want | Download |
|---|---|
| The window, on Linux | `.deb` (Debian, Ubuntu), `.rpm` (Fedora, RHEL), or `.AppImage` (anything) |
| The window, on macOS | `.dmg` — one file, both Apple and Intel machines |
| The window, on Windows | `-setup.exe` to double-click, or `.msi` to deploy |
| The command line | `ephemeral-<version>-<target>.tar.gz`, or `.zip` on Windows |
| To build a phone app on it | `Ephemeral-<version>-ios.xcframework.zip`, or `ephemeral-<version>-android.tar.gz` |

Full instructions, including how to put `ephemeral` on your `PATH`, are in
[docs/install.md](https://github.com/JGalego/Ephemeral/blob/main/docs/install.md).

## There is no phone app yet

The two mobile downloads are the engine, not an application: a static library
and a C header, for somebody writing the Swift or Kotlin shell around them.
There is nothing here to install on a phone.

What the library does on a device is **generate** — a sentence becomes an
application, its source written to the phone, using the app's own HTTPS client
and a credential from Keychain or Keystore. What it deliberately does not do is
build or run that application: that needs a sandbox no phone has, and running
generated code outside one is the thing Ephemeral exists to prevent. The
reasoning is in
[ADR-0017](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md),
and the contract is documented in the header itself.

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
