## Installing

| You want | Download |
|---|---|
| The window, on Linux | `.deb` (Debian, Ubuntu), `.rpm` (Fedora, RHEL), or `.AppImage` (anything) |
| The window, on macOS | `.dmg` — one file, both Apple and Intel machines |
| The window, on Windows | `-setup.exe` to double-click, or `.msi` to deploy |
| The command line | `ephemeral-<version>-<target>.tar.gz`, or `.zip` on Windows |

Full instructions, including how to put `ephemeral` on your `PATH`, are in
[docs/install.md](https://github.com/JGalego/Ephemeral/blob/main/docs/install.md).

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
