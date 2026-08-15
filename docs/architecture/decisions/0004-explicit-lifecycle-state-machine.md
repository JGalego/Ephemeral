# ADR-0004: Model the application lifecycle as an explicit event-driven state machine

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation

## Context

An Ephemeral application passes through planning, generation, building,
validation, possibly several rounds of automated repair, running, pausing,
archiving and deletion — and it can be interrupted at almost any point by a
permission request, a failure, or the user cancelling.

Three requirements pull on this at once:

- **The UI must explain the system.** Design principle 7 says the UI explains
  behaviour rather than hiding it. A user must be able to see *"this app is
  BUILDING because Ephemeral is installing its runtime"*, not a spinner. That
  means the state, the reason and the responsible actor have to be first-class
  data, not log lines.
- **It must be safe under an autonomous agent.** A repair loop is a program that
  drives this machine. If transitions are implicit, a loop can wander into a
  state nobody designed — including one where an app runs before its permissions
  were resolved.
- **It must survive a crash.** The machine is persisted, and a process that dies
  mid-build must come back to a state that is both valid and honest.

The failure mode to avoid is well known: a `status: String` field, or a large
enum whose transitions are whatever the call sites happen to do.

## Decision

**An explicit, event-driven, total state machine in `ephemeral-core`.**

- A closed `LifecycleState` enum, each state classified as *transient* (work in
  progress), *stable* (waiting for a human or a schedule) or *terminal*.
- A closed `LifecycleEvent` enum. Events drive transitions; states never change
  by assignment.
- `transition(state, event)` is a **total function**: every pair yields either
  the new state or a typed `IllegalTransition` error naming both. There is no
  default arm that silently ignores an unexpected event.
- Every event carries an **authorised actor set**. `Delete` requires
  `Actor::User`. The generation agent cannot mark its own output `Ready`. This
  is checked in the core, so an agent — or a prompt injection steering one —
  cannot transition its way past a control.
- Every accepted transition appends a `Transition` record: previous state, new
  state, event, actor, reason, timestamp, metadata, and structured error
  information where applicable. The history is persisted and is the data the UI
  renders.
- Interruptions (`PermissionRequired`, `Blocked`) record the state to resume to,
  so resumption is deterministic rather than reconstructed by guesswork.
- Repair is bounded in the machine itself: the repair counter lives with the
  state, and exhausting it transitions to a failure state rather than looping.

## Alternatives considered

### A status string or a flat enum with implicit transitions

Trivial to write, and it is what most systems have. Rejected because it makes
every illegal transition a runtime possibility, provides nothing for the UI to
explain, and gives an autonomous loop no guardrail at all. Explicitly named as an
anti-goal in the product brief.

### A workflow/orchestration engine (Temporal or similar)

Durable execution, retries, timers and history for free — a genuinely good fit
for a build/repair loop with bounded iterations. Rejected because it introduces a
server dependency into a product whose principle 9 is local-first and whose
principle 10 is no unnecessary cloud dependency, and because the history it
records is execution-shaped rather than user-shaped: we would still be building
our own domain history for the UI on top of it. Worth reconsidering only if
cloud execution becomes primary.

### A statechart library with hierarchical and parallel states

Handles the interruption problem elegantly — `PERMISSION_REQUIRED` is naturally
an orthogonal region rather than a state that must remember where to return to —
and would let us drop the explicit resume field. Rejected as premature: our
interruption set is small enough that an explicit resume state is simpler to
read, to persist and to test, and hierarchical states are considerably harder to
render honestly in a UI aimed at non-experts. Revisit if the interruption
matrix grows.

### An event-sourced aggregate, with state derived from the log

Only append events; compute the current state by folding history. Very close to
what we do, and it gives auditability for free. Rejected as the primary model
because every read would require a fold, and a corrupted or truncated history
would silently produce a *wrong but plausible* current state. We take the useful
half: state is stored explicitly, and the transition history is appended
alongside it, so the two can be cross-checked and a disagreement is detectable.

## Consequences

### What this makes easier

The UI gets a real answer to "what is happening and why" for free. Illegal
transitions are typed errors with names, so bugs surface at the transition
rather than three states later. The machine is exhaustively testable without
Docker, a network or a model. Crash recovery is well defined.

### What this makes harder

Adding a state means updating the transition table and its tests — deliberate
friction. Total functions over two enums mean a real matrix to maintain, and the
temptation to add a catch-all arm has to be resisted in review.

### What we are accepting

Some ceremony for simple transitions, and a transition table that grows with
the product. We accept the maintenance cost in exchange for the property that
"what states can this app be in, and how did it get here" is always answerable.

## Security implications

Direct. Actor authorisation on events is a structural defence against an
autonomous agent — or an injected one — escalating past a control: the agent
literally cannot emit `Delete`, `PermissionGranted` or the transitions that
would let unvalidated code run. Because `transition` is total, there is no
undefined behaviour to exploit, and the persisted history is the evidence trail
for "did this app ever run, and under whose decision".

## Revisit when

- The interruption matrix grows past what an explicit resume state handles
  cleanly (→ hierarchical statechart).
- Remote execution makes durable-workflow semantics the dominant concern (→
  revisit the orchestration engine).
