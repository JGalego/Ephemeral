# Installing Ephemeral

> **Nothing is published yet.** Ephemeral is in Phase 0 — the foundation. There
> are no installers, no packages and no releases, because there is not yet an
> application worth installing. This page describes what installation *will* be,
> and how to run what exists today.
>
> Track progress in [roadmap.md](roadmap.md). Packaged installers are a Phase 6
> deliverable.

## Running it today

From source. You need [Rust](https://rustup.rs) and nothing else.

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

That gives you the Phase 0 command line: creating application records,
inspecting them, moving them through their lifecycle, granting and revoking
permissions, reading the audit trail, and diagnosing your environment. Anything
needing a runtime or a model provider will tell you which phase it arrives in
rather than failing obscurely.

## What Ephemeral needs

**Rust**, to build from source. Once there are releases, you will not need it.

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

## What installation will look like

Phase 6, once there is a desktop application worth shipping:

| Platform | Planned |
|----------|---------|
| macOS | Signed and notarised `.dmg`, and Homebrew |
| Windows | Signed installer, and winget |
| Linux | `.deb`, `.rpm`, AppImage, and a Flatpak |
| iOS / Android | App Store and Play Store, subject to each store's rules |

Releases will carry checksums and, where the platform allows it, signatures. No
signing certificate or credential will ever be committed to this repository —
see [SECURITY.md](../SECURITY.md).

Building from source will stay a supported path, not a fallback.

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
