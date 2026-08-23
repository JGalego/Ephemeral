# ADR-0008: Provider-neutral generation, with a deterministic mock for CI

- **Status:** accepted
- **Date:** 2026-08-15
- **Deciders:** Ephemeral maintainers
- **Phase:** 0 — Foundation (trait), 2 — Generation (implementation)

## Context

Generation is where Ephemeral's value comes from and where its worst risks live.
Three forces shape the design.

**Providers change fast.** Model capabilities, prices and APIs move on a scale of
months. A product coupled to one provider's SDK inherits that churn, and its
users inherit that provider's outages, pricing and policies.

**Tests cannot depend on a model.** An LLM call is non-deterministic, costs
money, requires a credential, and needs the network. CI must not have any of
those properties, yet the build/repair loop — the most intricate control flow in
the product — is exactly what most needs testing.

**Model output is untrusted input.** A generated plan is a *request*, not a
command. If the agent can be steered — by a malicious prompt, a poisoned CSV, a
web page it read — then anything the agent can do directly, an attacker can do.

## Decision

**An `AgentProvider` trait, with a deterministic mock as a first-class
implementation.**

```text
AgentProvider (trait)
├── AnthropicProvider
├── OpenAIProvider
├── LocalProvider      (llama.cpp / Ollama-style, for offline generation)
└── MockProvider       deterministic, used by CI and E2E tests
```

The trait covers planning, code generation, tool execution, test execution,
inspection, error diagnosis, repair, iteration, cancellation and structured
outputs. Provider-specific concepts stay behind the trait; the core speaks only
in Ephemeral's own types.

Binding rules:

- **CI never makes a live model call.** Every test — unit, integration,
  security, end-to-end — runs against `MockProvider`, which returns fixed
  outputs for fixed inputs. A test that requires a real provider will not be
  merged. The rule is about the checks that gate a commit; a dispatch-only
  workflow that gates nothing and produces a document for a person to read
  (`many-apps.yml`) is outside it, and is dispatch-only for the three reasons
  above — it costs money, needs a credential, and needs the network.
- **The agent is not a privileged actor.** It cannot grant a permission, widen a
  resource limit, delete an application, or emit the lifecycle events reserved
  to the user (ADR-0004). Enforced in the core, never in a prompt.
- **Structured outputs, validated.** The agent returns typed, schema-validated
  structures that are parsed and checked; free-form text is never interpreted as
  an instruction to the system.
- **Everything is bounded.** Iterations, wall-clock, CPU, memory, artifact size,
  network and spend all have hard limits, and the user can cancel at any point.
- **Generated code executes only in a sandbox** (ADR-0005), never with the
  privileges of the process that generated it.
- **Reasoning and actions are audited** at a level useful for debugging and
  incident review, with redaction on the write path so secrets and credentials
  cannot enter the record.

## Alternatives considered

### Couple directly to one provider's SDK

Fastest path, and gives access to provider-specific features (caching,
structured output modes, tool formats) without abstraction loss. Rejected on
product grounds: it makes an outage or a pricing change at one vendor an
Ephemeral outage, and it forecloses the local-model path that offline generation
requires. Note the real cost of the abstraction — we will lag provider features
— and we accept it.

### Use a third-party agent framework (LangChain-style) as the abstraction

Provider abstraction, tool calling, retries and agent loops, all off the shelf.
Rejected because the build/repair loop's bounds, cancellation and actor
restrictions are security controls, and we are not willing to have them live in
a fast-moving third-party dependency. The loop is small; the control over it is
the point.

### Record and replay real provider responses (VCR-style cassettes) for CI

More realistic than a hand-written mock, and catches provider-shape regressions.
Genuinely useful, and likely to be added as a *supplementary* suite. Rejected as
the primary mechanism because cassettes must be regenerated with a real
credential, they encode one model's behaviour at one moment, and they make the
test suite's determinism depend on a recording nobody wants to refresh.

### Run a small local model in CI instead of a mock

Deterministic-ish with a fixed seed, and end-to-end realistic. Rejected: slow,
large, still not reliably deterministic across hardware, and it tests the model
rather than our orchestration.

## Consequences

### What this makes easier

Swapping or adding providers without touching the core. Fully deterministic CI,
including the end-to-end journey. Offline generation with a local model. A
single place to enforce the bounds and the actor restrictions.

### What this makes harder

The abstraction lags provider-specific features, and each new provider needs its
own conformance work. A mock that drifts from real provider behaviour will hide
bugs — the mock's fidelity has to be maintained deliberately, not assumed.

### What we are accepting

We are trading some model capability for portability and testability, and taking
on the ongoing job of keeping the mock honest.

## Security implications

This is the primary structural answer to prompt injection. Because the agent is
not an authorised actor for grants, deletions or limit changes, a successfully
injected agent still cannot escalate privilege — it can only produce output that
a user must approve and that a sandbox will contain. Treating model output as
data rather than instruction, validating structured outputs, bounding every
axis, and redacting on the write path are all part of the same posture:
**generated code and generated plans are untrusted, regardless of which provider
produced them** — including a compromised one.

## Revisit when

- A provider capability we need cannot be expressed through the trait without
  distortion.
- Mock/real divergence causes a bug that reaches users (→ add the replay suite).
