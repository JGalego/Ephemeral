# Installing Ephemeral

> **These builds are not signed.** Every installer below is unsigned and
> un-notarised, so macOS and Windows will warn you that they cannot tell who
> built it. They are right, and each section says exactly what you will see and
> what to do about it. Do not click past those warnings out of habit.

Installers are built by [the release pipeline](../.github/workflows/release.yml)
for every tag, on Linux, macOS and Windows, and attached to
[the release](https://github.com/JGalego/Ephemeral/releases) with checksums.

## The desktop window

### Linux

Three formats, because Linux is not one thing:

```bash
sudo apt install ./Ephemeral_<version>_amd64.deb      # Debian, Ubuntu, Mint
sudo dnf install ./Ephemeral-<version>-1.x86_64.rpm   # Fedora, RHEL, openSUSE

chmod +x Ephemeral_<version>_amd64.AppImage           # anything else
./Ephemeral_<version>_amd64.AppImage
```

The `.deb` and `.rpm` pull in the WebKit runtime they need. The AppImage does
not install anything: it is a single file you can run and delete.

Linux does not check signatures on any of these, so there is no warning to
click past — which is a statement about Linux, not about this build.

### macOS

Open the `.dmg` and drag Ephemeral to Applications. Then, the first time:

macOS will refuse to open it, saying either that the developer cannot be
verified or — misleadingly — that the app is "damaged". It is not damaged. It
is unsigned, and an unsigned application that arrived from the internet is
quarantined.

**Right-click the app and choose Open**, which offers a way through that
double-clicking does not. If macOS still refuses:

```bash
xattr -dr com.apple.quarantine /Applications/Ephemeral.app
```

That command removes the quarantine flag. Run it on this application because
you decided to trust this download, not because a page told you to — it is
exactly the command somebody would like you to run on their malware.

One `.dmg` covers both Apple silicon and Intel.

### Windows

Two installers, and which you want depends on who is installing:

- **`Ephemeral_<version>_x64-setup.exe`** — double-click. This is the one you
  want.
- **`Ephemeral_<version>_x64_en-US.msi`** — for deploying across a fleet with
  Group Policy or Intune.

SmartScreen will say "Windows protected your PC" and hide the Run button behind
**More info** → **Run anyway**. That warning means Windows has never seen this
publisher's signature, which is true: there is not one.

## The command line

Download the archive for your platform, unpack it, and put `ephemeral`
somewhere on your `PATH`:

```bash
tar -xzf ephemeral-<version>-x86_64-unknown-linux-gnu.tar.gz
sudo install -m755 ephemeral-<version>-*/ephemeral /usr/local/bin/ephemeral

ephemeral doctor
```

On Windows, unzip it and add the folder to `PATH`, or drop `ephemeral.exe`
somewhere already on it.

Builds are published for x86-64 and ARM on all three platforms.

### Verifying what you downloaded

```bash
sha256sum -c SHA256SUMS.txt --ignore-missing
```

`shasum -a 256 -c` on macOS; `Get-FileHash <file> -Algorithm SHA256` on
Windows.

A checksum tells you the file arrived intact. It does not tell you who made it.
Those are different claims, and until these builds are signed, only the first
one is available.

## Building from source

Supported, not a fallback. You need [Rust](https://rustup.rs) and nothing else.

```bash
git clone https://github.com/JGalego/Ephemeral.git
cd Ephemeral
./scripts/bootstrap          # macOS / Linux
scripts\bootstrap.ps1        # Windows

cargo run -p ephemeral-cli -- doctor
```

To put `ephemeral` on your path:

```bash
cargo install --path crates/ephemeral-cli --locked
ephemeral doctor
```

To build the desktop window from source as well, see
[development.md](development.md) — it is its own workspace, and on Linux it
needs `libwebkit2gtk-4.1-dev librsvg2-dev patchelf`.

## What Ephemeral needs

**Rust**, only if you are building from source. The installers above carry
everything they need.

**Docker is optional.** It is the desktop default for running generated
applications, but Ephemeral is designed to work without it and never treats its
absence as a failure ([ADR-0005](architecture/decisions/0005-docker-first-runtime-abstraction.md)).
`ephemeral doctor` will tell you whether it is present, whether the daemon is
reachable, and what to do if not — and Ephemeral will always ask before
installing anything.

**Nothing else.** No account, no server, no network connection for anything
except generation. Desktop Ephemeral is local-first: local state, local
execution, local logs.

## Where your files go

Ephemeral puts its files where each operating system expects, rather than
scattering dotfiles:

| Platform | Location |
|----------|----------|
| Linux | `$XDG_DATA_HOME/ephemeral`, or `~/.local/share/ephemeral` |
| macOS | `~/Library/Application Support/Ephemeral` |
| Windows | `%APPDATA%\Ephemeral` |

Override it with `EPHEMERAL_HOME` or `--home`. That is also the clean way to try
Ephemeral without touching anything you care about:

```bash
EPHEMERAL_HOME=/tmp/ephemeral-scratch ephemeral create "compare two CSV files"
```

Inside, the layout is meant to be opened and understood:

```text
apps/<app-id>/
  manifest.json    what the application is and what it may do
  source/          generated source
  build/           build output
  runtime/         runtime scratch, destroyed on teardown
  data/            the application's own data
  logs/            build, test and runtime logs
  artifacts/       exports and reports
trash/             deleted applications, until purged
permissions.json   every permission decision, including revoked ones
audit.json         the append-only security record
```

**Secrets are not in there.** They live in your platform's secure storage —
Keychain, Credential Manager, Secret Service — and are injected into runtimes as
values the manifest and the interface never see.

## Uninstalling

Because everything is local and in one place, removing Ephemeral is removing a
directory:

```bash
rm -rf "$(ephemeral doctor | grep 'storage at' | ...)"   # or just the path above
cargo uninstall ephemeral-cli
```

If you want to destroy applications properly first — including anything a
runtime might still hold — use `ephemeral purge <app> --yes` on each, which is
explicit, irreversible and audited.

## What is shipped, and what is not

| | Today | Still to come |
|---|---|---|
| Linux | `.deb`, `.rpm`, AppImage | Flatpak |
| macOS | Universal `.dmg` | Signing, notarisation, Homebrew |
| Windows | NSIS `.exe`, `.msi` | Authenticode signing, winget |
| CLI | Archives for x86-64 and ARM | Homebrew, winget |
| iOS / Android | — | App Store and Play Store, subject to each store's rules |

**Signing is the significant gap.** It needs an Apple Developer account and a
Windows code-signing certificate, both of which are paid identities belonging to
a person or an organisation rather than to a repository. Until then, the
warnings described above are correct and should be read rather than dismissed.

No signing certificate or credential will ever be committed to this repository —
see [SECURITY.md](../SECURITY.md). When signing does arrive, the keys will live
in repository secrets and the release workflow will use them without ever
printing them.

## Trouble

Start with:

```bash
ephemeral doctor
```

It checks whether Ephemeral can write where it intends to, whether a container
runtime is present and reachable, whether your applications are readable, and
whether the audit record is intact — and every check that fails says what would
fix it.

If that does not help, open an issue with the `doctor` output. If it is a
security problem, do **not** open an issue: report it privately, as described in
[SECURITY.md](../SECURITY.md).
