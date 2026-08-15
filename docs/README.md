# Documentation

## Start here

| | |
|---|---|
| [concepts.md](concepts.md) | The vocabulary: intent, application, principal, actor, scope, retention |
| [roadmap.md](roadmap.md) | What exists today, what is next, and what is deliberately not being built yet |

## The design

| | |
|---|---|
| [../ARCHITECTURE.md](../ARCHITECTURE.md) | How the system is put together |
| [architecture/decisions/](architecture/decisions/) | Why it is put together that way, and what was rejected |
| [lifecycle.md](lifecycle.md) | The application lifecycle state machine |
| [permissions.md](permissions.md) | Both permission systems, in detail |
| [manifest.md](manifest.md) | The versioned application manifest |
| [sandbox.md](sandbox.md) | What confines a running application, and what it will not do |
| [sharing.md](sharing.md) | Design: giving an app to somebody else, publishing it, and shared instances |

## Working on it

| | |
|---|---|
| [development.md](development.md) | Bootstrap, the loop, testing, style, dependencies |
| [../CONTRIBUTING.md](../CONTRIBUTING.md) | Commits, ADRs, definition of done |
| [assets/](assets/) | The logo and the banner, and how to regenerate them |

## Using it

| | |
|---|---|
| [install.md](install.md) | Installation, where files go, uninstalling |
| [../SECURITY.md](../SECURITY.md) | The security model, and how to report a vulnerability |

## Not written yet

- **`security/threat-model.md`** — required before the MVP is declared complete,
  and a Phase 6 deliverable. Scope is listed in
  [SECURITY.md](../SECURITY.md#threat-model).
- **`api.md`** — the versioned Core API between clients and the core, once there
  is more than one client to hold it honest.
