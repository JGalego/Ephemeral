# ADR-0020: The host chooses the provider, on every platform

- **Status:** accepted
- **Date:** 2026-08-23
- **Deciders:** Ephemeral maintainers
- **Phase:** 5 — Clients

## Context

[ADR-0008](0008-agent-provider-abstraction.md) made generation provider-neutral.
[ADR-0019](0019-openai-compatible-and-a-local-model.md) added a second wire
format so that one provider reaches OpenAI, Groq, Together, a company gateway or
a model on the next desk. On the desktop and in the terminal that is real: the
provider is an argument, the endpoint and the model are environment variables,
and nobody is locked to a vendor.

On a phone none of it was reachable. `ephemeral-ffi` held one field —

```rust
pub struct Ephemeral {
    provider: AnthropicProvider,
    …
}
```

— and the C ABI's transport callback took a bare `api_key`, leaving the host to
write `x-api-key` and `anthropic-version` itself. Both phones did. So the shape
of the boundary, not any configuration anywhere, decided that a handset talks to
Anthropic and to nothing else.

That was a considered decision at the time, and the code said so: wiring a
second provider meant passing a whole header set across a published ABI, which
is a change to a contract rather than an internal detail. The reasoning was
sound and the conclusion was wrong. A platform is not entitled to pick somebody's
vendor. Generating is one HTTPS request; which company answers it is a decision
belonging to the person whose bill it is, and "the ABI would need changing" is a
cost, not a principle.

It is also the sharpest version of a rule this repository already holds
everywhere else: **there is one Ephemeral, and a client is a client.** A phone
that could offer fewer *capabilities* than a desktop is honest — it has no
container runtime and cannot build anything. A phone that offers fewer *choices*
about who sees the user's intent is not the same kind of limitation. The first
is physics; the second was our code.

## Decision

**The header set crosses the boundary.** `EphemeralHttpSend` takes
`headers_json` — `[{"name":…,"value":…}, …]`, in the order the provider composed
them — instead of one credential. A host sets exactly those and adds nothing.
Which headers a service wants is the provider's knowledge, and a transport that
knew any of them was a transport that belonged to one vendor.

**The host chooses the provider.** Three new entry points:

| | |
|---|---|
| `ephemeral_set_provider` | `{"provider":…, "base_url":…, "model":…, "ceiling":…}` |
| `ephemeral_provider` | what is chosen now, as the same JSON |
| `ephemeral_providers` | the catalogue, needing no handle |

Only `provider` is required; anything absent means that provider's own default.
A name that is not in the catalogue is **refused**, never defaulted past —
generating with a company somebody did not choose is worse than not generating.

**The catalogue is read, not written down.** A host builds its picker from
`ephemeral_providers`, which reports each provider's one-line description,
whether it needs a credential, which fields it reads, and what it defaults to. A
list of providers hardcoded in an application is a list that is wrong the moment
one is added, and an application does not ship on the engine's schedule.

**The credential stays separate.** It is not part of the choice, does not appear
in `ephemeral_provider`'s answer, and still arrives through
`ephemeral_set_credential` from the platform's secure store. That separation is
what lets the choice live in ordinary preferences — `UserDefaults`,
`SharedPreferences` — while the key stays in the Keychain or the Keystore.

**A provider is built per call, not held.** The version that held one built
provider had to swap it through a placeholder transport whenever a credential
arrived, and could not change anything else about it at all. Building it from
the recorded choice at the moment of use is what makes "change the provider"
a matter of writing down a different choice.

**`local` is not offered on a phone.** It exists to keep an intent on the machine
that generated it and refuses any endpoint that is not loopback — which on a
handset means a model server running on that handset. Anything else somebody
means by "local" is another machine, and that is `openai` with a base URL, which
is what it honestly is. The mock *is* offered: seeing the whole flow work before
handing a phone a credential is worth a menu entry.

**`ANTHROPIC_BASE_URL` and `ANTHROPIC_MODEL` exist too.** The Anthropic provider
was the one hosted provider whose model was fixed, justified by its endpoint
being fixed. Once the endpoint can be pointed at a gateway, a fixed model name
is just a gap.

## Consequences

**Breaking, and deliberately so.** `EphemeralHttpSend` changed shape. Any host
built against the old signature compiles into a transport that receives JSON
where it expects a key and sends it as `x-api-key`. Both hosts in this
repository are updated in the same change; anybody else's is not, and the header
says what happened and why.

**The default is unchanged.** A host that never calls `ephemeral_set_provider`
gets Anthropic, as before. An upgrade does not silently move somebody's traffic
to another company.

**One more thing a phone can get wrong.** A base URL and a model name are two
new ways to be misconfigured, and the failure is a service's own authentication
or model-not-found error rather than anything Ephemeral can phrase better. The
mitigation is the catalogue: the fields shown are the ones the chosen provider
actually reads, pre-filled with what it would use anyway.

**The tests that matter are at the boundary.** `a_phone_can_generate_with_groq`
drives the real C ABI through a fake host and asserts the request went to the
URL that was chosen and carried `Authorization: Bearer` rather than
`x-api-key`; `the_default_reaches_anthropic` asserts the other half. Neither
could have passed before this change, and that is the point of having both.

## Alternatives considered

### Leave it, and document the limitation

Cheapest, and what the code already did. Rejected: a documented vendor lock is
still a vendor lock, and this one had no technical justification left — the
second wire format already existed and already compiled without a subprocess,
which was the only thing that ever made it hard.

### Keep the ABI and add an "Anthropic-compatible" mode

Have the host keep writing headers, with a flag saying which set. Rejected: it
puts a table of every provider's header conventions in every host, in a language
that is not where the provider lives, updated on a different release cycle.
That is the same mistake one level up.

### A second callback for OpenAI-shaped requests

Rejected for the same reason with more symbols: the boundary would grow one
function per wire format forever, and each one would still be the ABI knowing
something only a provider should know.
