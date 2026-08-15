# Security Policy

Ephemeral generates, builds and executes code that no human reviewed, from
instructions written in natural language, on a user's own machine. Security is
not a feature of this product — it is the product's central engineering problem.

## Reporting a vulnerability

Please report security issues **privately**:

- Use GitHub's [private vulnerability reporting](https://github.com/JGalego/Ephemeral/security/advisories/new)
  on this repository.
- Do not open a public issue for a vulnerability.

Please include: what you found, how to reproduce it, the impact you believe it
has, and your platform and version. We will acknowledge within a few days and
keep you updated as we work on a fix. We are happy to credit you in the advisory
unless you prefer otherwise.

While Ephemeral is pre-1.0 there is no long-term-support branch; fixes land on
`main` and in the next release.

## Scope

In scope, and taken seriously:

- Escaping an application sandbox (container escape, mount escape, host access)
- Any path by which a generated application obtains a permission it was not
  granted
- Any path by which a generated application reads another application's data
- Exposure of Ephemeral's own credentials or a user's secrets to a generated
  application, a log, a manifest, or the UI
- Privilege escalation of Ephemeral's meta-permissions without user consent
- Prompt injection that results in a security-relevant action (permission grant,
  data exfiltration, code execution) rather than merely a low-quality output
- Persistence of an application's runtime access after deletion
- Supply-chain issues in our build, release or dependency pipeline
- Tampering with the audit log without detection

Out of scope: quality of generated code that does not cross a security boundary;
denial of service against your own machine by your own configured limits;
findings that require an already-compromised host or a malicious OS.

## Security model, in short

Read [ARCHITECTURE.md](ARCHITECTURE.md) for the full picture. The load-bearing
assumptions:

### Generated code is untrusted

An LLM wrote it. That is not evidence of good intent or correctness. Generated
code is treated exactly as code downloaded from a stranger: it runs sandboxed,
non-root, with an explicit and minimal set of capabilities, and it is never
given ambient authority.

### Two permission systems, never merged

Ephemeral's own permissions (**meta-permissions**) and each generated app's
permissions are separate types with separate ledgers. A generated app inherits
nothing from Ephemeral. Ephemeral being allowed to read your home directory does
not let an app read a single file in it.

### Default deny

The permission ledger denies by default. Only an explicit, unexpired,
unrevoked `Allow` naming that exact principal and that exact permission permits
an operation. An explicit `Deny` always wins over an `Allow`.

### Least privilege, narrowly scoped

Permissions name the narrowest scope that works: a directory, not a filesystem;
a host, not the internet. The user's home directory is never mounted into a
generated container. Network access is denied unless granted, and granted egress
is allow-listed.

### Secrets never touch the app

Secrets live in platform-native secure storage — macOS Keychain, Windows
Credential Manager/DPAPI, Linux Secret Service, iOS Keychain, Android Keystore.
They are injected into runtimes as environment values that never appear in the
app manifest, the UI, the audit log, or any log file. Redaction runs on the
write path, not as a display-time filter.

### Prompt injection is a security problem

A CSV file, a web page or a filename can contain text aiming to redirect the
generation agent. Ephemeral treats all model input as untrusted data and never
as instruction: the agent cannot grant permissions, cannot widen its own limits,
cannot delete applications, and cannot cause a privileged operation without the
user's explicit decision. Actor restrictions on lifecycle events and permission
grants are enforced in the core, not in the prompt.

### Nothing important is silent

Every security-sensitive operation lands in an append-only, hash-chained audit
log: permission requests and decisions, container creation, mounts, exposed
ports, secret *access* (never values), deletions and purges.

### Bounded by construction

Autonomous repair loops are bounded on iterations, wall-clock, CPU, memory,
storage, network and cost. A runaway agent loop is a security and financial
issue, and the user can cancel at any time.

## Threat model

A full threat model covering malicious generated code, prompt injection,
malicious external content, dependency and supply-chain attacks, container
escape, privilege escalation, secret exfiltration, a compromised AI provider, a
compromised runtime, malicious plugins, filesystem and network attacks, resource
exhaustion and persistence after deletion is a required deliverable before the
MVP is declared complete. It will live at `docs/security/threat-model.md` and is
tracked as a Phase 6 item in [the roadmap](docs/roadmap.md).

## Supply chain

- Dependencies are pinned and committed lockfiles are authoritative.
- Automated dependency updates and vulnerability scanning run in CI.
- SBOMs and checksums are produced for releases; artifacts are signed where the
  platform permits.
- Container base images are minimal and refreshed regularly.
- Dependencies chosen by *generated code* are not blindly executed — they are
  resolved and installed inside the app's sandbox, under the app's permissions,
  never Ephemeral's.

## What we will never do

- Store credentials, tokens or private keys in this repository.
- Ship a permission dialog that does not say what is asking, what it wants and
  why.
- Escalate a permission silently.
- Mount a user's whole home directory into a generated container.
- Expose Ephemeral's own credentials to a generated application.
- Let generated code modify Ephemeral's own installation without explicit
  authorisation.
