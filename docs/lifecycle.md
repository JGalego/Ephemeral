# The application lifecycle

Every application Ephemeral creates moves through one explicit state machine.
It is the thing the interface renders, so a user can read *why* their app is
doing something rather than watch a spinner and guess.

Rationale and rejected alternatives:
[ADR-0004](architecture/decisions/0004-explicit-lifecycle-state-machine.md).
To see it from the terminal: `ephemeral states`.

## The shape of it

```text
        REQUESTED
            │ plan
            ▼
        PLANNING ──────────────┐
            │ plan ok          │
            ▼                  │  any working state can be
        GENERATING             │  interrupted by
            │ generated        │
            ▼                  ├──▶ PERMISSION_REQUIRED ──▶ (resume)
         BUILDING ◀────┐       │
            │ built    │       ├──▶ BLOCKED
            ▼          │       │
        VALIDATING     │       └──▶ CANCELLED
          │      │     │
     pass │      │ fail│ repair (bounded)
          │      ▼     │
          │  VALIDATION_FAILED ──▶ REPAIRING ──┘
          ▼
        READY ⇄ STARTING ⇄ RUNNING ⇄ PAUSED
          │                   │
          │                   └──▶ UNHEALTHY ──▶ RUNTIME_FAILED
          ▼
       ARCHIVED ──restore──▶ READY
          │
          ▼
       DELETED  ──restore──▶ ARCHIVED   (until purged)
```

## What makes it trustworthy

**Total.** `LifecycleState::next` is defined for every one of the 20 × 31 state
and event pairs. An event that has no meaning in a state is a typed error naming
both, never a silent no-op. There is no catch-all arm.

**Explicit.** The table is written out in `LifecycleState::outgoing`, so it can
be read top to bottom and agreed with rather than inferred from call sites.

**Authorised.** Every event names the actors that may raise it, checked here
rather than requested in a prompt. See [below](#who-may-do-what).

**Bounded.** The repair budget lives with the machine, so the autonomous loop
terminates by construction rather than by convention.

**Recorded.** Every applied transition appends the previous state, the new
state, the event, the actor, the reason, the time, metadata and structured error
information.

**Persisted.** The whole machine serialises, so a crash mid-build resumes into a
state that is both valid and honest.

## The states

Each state has a *kind*, which is what an interface keys its treatment off. A
user should never have to learn twenty names to know whether they need to do
something.

| Kind | Means | States |
|------|-------|--------|
| Working | Ephemeral is busy; wait (or cancel) | `requested`, `planning`, `generating`, `building`, `validating`, `repairing`, `starting`, `stopping` |
| Awaiting you | Nothing happens until you decide | `permission_required`, `blocked` |
| Idle | Built and available, not running | `ready`, `paused` |
| Active | Running now | `running` |
| Attention | Went wrong or was stopped | `unhealthy`, `build_failed`, `validation_failed`, `runtime_failed`, `cancelled` |
| Archived | Put away, restorable | `archived` |
| Deleted | Ended; recoverable until purged | `deleted` |

Every state carries plain-language text. `building` is not shown as "BUILDING"
but as:

> Ephemeral is building the app and setting up what it needs to run.

paired with the reason recorded on the latest transition, giving the sentence
the product brief asks for: *"This app is BUILDING because Ephemeral is
installing its runtime."*

## Rules worth knowing

**Deletion is always available.** Every state except `deleted` accepts `delete`.
A user must be able to stop an application whatever it is doing.

**Only working states can be interrupted.** An application that is `ready` or
`running` is not waiting on Ephemeral, so it cannot be diverted into
`permission_required`.

**Interruptions remember where they were.** `permission_required` and `blocked`
record the state to resume into, so granting or unblocking continues
deterministically rather than restarting. The recorded state is validated on the
way back: a resume value that is not an interruptible state is refused, so
corrupted state cannot return an application to `running` without the runtime
ever being involved.

**A repair is always re-tested.** The only way out of `repairing` towards
success is back through `building`, which forces `validating` again. A fix is
never assumed to have worked.

**Restore from `deleted` leads to `archived`, not `ready`.** Recovering from the
trash restores the record, not the ability to run; you restore again to make it
runnable.

**Some legal transitions are still refused.** `AppManifest::apply` validates the
result and rolls back if it would be invalid. Restoring moves an app to `ready`,
which requires a runtime — and an app cancelled during planning never got one.
The state machine cannot know that; the manifest can.

## Who may do what

Authorisation is attached to the event, not to a role hierarchy. The rules, and
the reasoning:

| | |
|---|---|
| **Decisions belong to the user.** | `permission_granted`, `permission_denied`, `cancel`, `restore` and `delete` are `user`-only. No autonomous component makes a choice you would expect to make yourself. |
| **The agent never signs off its own work.** | `validation_passed` excludes `agent`, so the thing that wrote the code cannot declare it correct. |
| **Execution facts come from the runtime.** | `started`, `stopped`, `start_failed`, `runtime_crashed` and the health events are reported by whatever is actually running the code, not asserted by the orchestrator. |
| **Expiry is the system's.** | Retention sweeps are not user actions and are attributed honestly. |

This is the structural defence against prompt injection. An agent that has been
successfully steered by something it read still cannot approve, authorise or
destroy — because the restriction is a check in the core, not an instruction in
a prompt.

```console
$ ephemeral states my-app-3f2a1b9c
What you can do from here
  cancel               cancelled the work
  delete               deleted the app
```

The same question asked as the agent returns nothing at all.

## The transition record

Every transition keeps:

| Field | Why |
|-------|-----|
| `from`, `to` | what changed |
| `event` | what caused it |
| `actor` | who says so |
| `reason` | in language a person can read |
| `at` | when, in UTC |
| `metadata` | build number, image digest, exit code, duration |
| `error` | a stable code, a message, and detail for a developer |

`reason` is what turns *"Building"* into *"Building — Ephemeral is installing its
runtime"*, so it is worth writing properly. `error` is separate from it so
failures can be matched, aggregated and compared across runs without parsing
prose.

```console
$ ephemeral logs my-app-3f2a1b9c
2026-08-15 14:44:02  Deleted (requested)
  You deleted the app — deleted from the command line
2026-08-15 14:44:02  Archived (deleted)
  You restored the app — restored from the command line
```

## The repair budget

`REQUESTED → … → VALIDATING → VALIDATION_FAILED → REPAIRING → BUILDING → …`

Repair is bounded at three attempts by default. The counter lives with the
machine, and a `repair` event raised after the budget is spent is refused with
`RepairBudgetExhausted` — leaving the application exactly where it was rather
than looping.

`retry` resets the budget, because starting over is a fresh attempt rather than
a continuation.

An unbounded repair loop is a security and cost problem, not merely an
annoyance, which is why the bound is in the domain model rather than in the
orchestrator that drives it.
