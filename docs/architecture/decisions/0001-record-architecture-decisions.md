# ADR-0001: Record architecture decisions

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation

## Context

Ephemeral is a system with an unusually high density of consequential, hard-to-
reverse decisions: which framework spans five platforms, how permissions are
modelled, what a sandbox boundary is, what a manifest promises, where generated
code executes on mobile. Most of these decisions are only *defensible* if you
know what else was on the table.

Without a record, three things happen. Contributors re-litigate settled
questions. Reviewers cannot tell an intentional constraint from an accident.
And, worst for a security-sensitive product, a boundary gets weakened by someone
who did not know it was load-bearing.

## Decision

We keep Architecture Decision Records in `docs/architecture/decisions/`, one
Markdown file per decision, numbered sequentially, using
[the template](0000-template.md).

An ADR is required for any decision that establishes a security boundary,
selects a major framework or protocol, defines a persisted format, or would
otherwise have to be reverse-engineered from the code by a future contributor.
Routine implementation choices do not need one.

ADRs are immutable once accepted. A decision that changes gets a *new* ADR that
supersedes the old one; the old one stays, marked superseded and linked forward.
The history of what we believed and when is part of the record.

## Alternatives considered

### A wiki or an external design-doc tool

Better editing experience and easier cross-linking. Rejected because the
decisions would drift out of sync with the code that implements them: a wiki is
not reviewed alongside a pull request, so nothing forces the record to be
updated when the reality changes. ADRs in-tree are reviewed in the same diff as
the code.

### Comments in the source

Closest to the code, so least likely to rot. Rejected because a decision that
spans crates has no natural home in any one file, and because the *rejected*
alternatives — the most valuable part — do not belong in a function's doc
comment.

### No formal record; rely on pull request discussion

Zero overhead. Rejected because PR discussion is unsearchable in practice,
attaches to a diff rather than to a concept, and disappears from view the moment
the branch merges.

## Consequences

### What this makes easier

Onboarding, review, and saying "no" to a change with a reason rather than a
preference. Security review in particular: the threat model can cite the ADR
that established each boundary.

### What this makes harder

Every significant decision now costs an extra document, and the discipline has
to hold when the project is moving fast.

### What we are accepting

Some ADRs will be written for decisions that turn out not to matter. That is
cheaper than the alternative failure mode.

## Security implications

Positive and direct. Security boundaries that are written down are boundaries
that can be tested, audited and defended in review. An undocumented boundary is
one someone will eventually remove by accident.

## Revisit when

Never, realistically. If the ADR process itself becomes the bottleneck, tighten
the "requires an ADR" threshold rather than abandoning the practice.
