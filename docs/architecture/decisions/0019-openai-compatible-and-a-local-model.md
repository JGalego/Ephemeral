# ADR-0019: One OpenAI-compatible wire format, and a local provider that cannot leave the machine

- **Status:** accepted
- **Date:** 2026-08-22
- **Deciders:** Ephemeral maintainers
- **Phase:** 2 — Generation

## Context

[ADR-0008](0008-agent-provider-abstraction.md) made generation provider-neutral
and named a local model as a supported shape. Nothing implemented one. The
[threat model](../security/threat-model.md) has said so plainly in T6 for as
long as it has existed: a hosted provider learns what the user asked for, the
mitigation available is choice rather than prevention, and the offline path is
the only real answer — "a local model, not a promise".

Two things have to exist for that sentence to stop being an IOU.

**A second wire format.** `ephemeral-provider-anthropic` speaks Anthropic's
Messages API. Nothing a person runs on their own machine speaks it. What they do
run — Ollama, llama.cpp's server, LM Studio, vLLM — speaks OpenAI's chat
completions API, and so do most hosted services that came after OpenAI. One
format therefore reaches both a hosted alternative to Anthropic and every local
server worth naming.

**A promise about where a request goes.** "Local" is not a wire format; it is a
destination. A provider called `local` that happily posted the user's intent to
`https://api.example.com` because a variable was set would be worse than no local
provider at all, because the name is what somebody would rely on.

There is a third force, from [ADR-0016](0016-real-providers-live-in-their-own-crates.md):
prompts are shared by every provider so that two cannot drift, and the untested
surface — the transport — stays one small module. Neither of those may be
loosened to add a provider.

## Decision

**`ephemeral-provider-openai` owns the chat completions wire format**, as a
`wire` module of pure functions, exactly as the Anthropic crate does. Its
endpoint, model and credential come from `OPENAI_BASE_URL`, `OPENAI_MODEL` and
`OPENAI_API_KEY`, so the same crate serves OpenAI and anything that copied it.

**`ephemeral-provider-local` is that wire format pinned to this machine.** It
depends on the OpenAI crate for the format and adds the one thing that makes it
a different provider: `endpoint::is_on_this_machine`, a pure function checked
before every request. An endpoint that is not loopback is refused, by name, with
`--provider openai` offered as the honest way to reach a model server somewhere
else. Its credential variable is `EPHEMERAL_LOCAL_API_KEY` rather than
`OPENAI_API_KEY`, so a hosted credential in the environment is never handed to a
process on the machine.

**The response ceiling's field name is a parameter.** OpenAI renamed
`max_tokens` to `max_completion_tokens`; the servers that copied the format
copied it before the rename. The hosted provider sends the new name, the local
one sends the old, and neither sends both — which OpenAI rejects.

## Alternatives considered

### One crate holding both providers

Fewer moving parts, and the shared wire format needs no dependency between
crates.

Rejected because the two differ in exactly the way that matters: one may send a
request anywhere and the other may not. Two types make that a property of the
type a caller chose, checked by the compiler, rather than a field somebody could
set wrongly — and it keeps ADR-0016's one-crate-per-provider shape, under which
`--provider local` is a thing a person selects rather than a mode.

### A local provider that copies the wire format instead of depending on it

Independent crates, no coupling.

Rejected for the reason the prompts are shared: two copies of a format drift,
and a request built one way here and another way there is how "the same
application, generated twice, differs" happens. The dependency is one-way and
the shared part is pure.

### Speak Ollama's own API instead of the OpenAI-compatible one

Native, and marginally richer — it exposes model loading and a keep-alive that
the compatibility layer does not.

Rejected because it would fit exactly one server. The compatibility endpoint
reaches Ollama, llama.cpp, LM Studio, vLLM and anything else that copied it, and
nothing Ephemeral asks for needs what the native API adds.

### Trust a name and a default rather than checking the endpoint

`local` defaults to loopback; a person who overrides it knows what they are
doing.

Rejected. The default is not the promise — a variable in a shell profile, a
`.env` copied from somewhere, or an endpoint that reads as loopback and is not
(`http://127.0.0.1@elsewhere.example/`) would each break it silently. A promise
worth making is one worth checking, and this one costs a pure function.

### Reach a model on another machine on the LAN under `local`

A model server on a machine in the same house is not a hosted provider, and
somebody will want this.

Rejected for now, because "this machine" is a line a person can reason about and
"the network I trust" is not. That configuration is available today through
`--provider openai` with a base URL, which is honest about what it is: a request
leaving the machine.

## Consequences

### What this makes easier

Generating without sending the intent anywhere — the roadmap's last open Phase 2
row, and the T6 mitigation the threat model has been describing in the future
tense. It also means a person who dislikes both hosted providers has a third
option that is not "write the code yourself".

Any OpenAI-compatible service — a hosted alternative, a company's own gateway, a
model behind a proxy — is now reachable without new code.

### What this makes harder

Three real providers rather than one, all of which have to keep working. The
mitigation is that they share the prompts, the parsing helpers and the
transport, so what is genuinely per-provider is a request envelope and where the
text sits in a reply.

### What we are accepting

**A small local model will often fail.** Generation asks for a whole application
as one valid JSON object, and models that fit comfortably on a laptop produce
malformed replies more often than a hosted model does. The loop's repair round
absorbs some of that; the rest a person sees as a refusal to act on an
unreadable reply, which is the correct behaviour and still a worse experience.
The documentation says so rather than implying parity.

**A local model is not a more trustworthy model.** It is a more private one. Its
output is validated identically, because privacy is not integrity.

**The endpoint check refuses some things that would have worked**, including
`127.1` and a bare unbracketed IPv6 address. Refusing a working configuration is
a nuisance; accepting a remote one breaks the only promise the provider makes.

## Security implications

This is the first mitigation Ephemeral has for T6 that is not "choose a
different company". With `--provider local`, the intent, the generated source
and the build output stay on the machine.

The strength of that claim rests on one pure function, so it is tested against
the URLs designed to defeat it: the loopback address as userinfo
(`http://127.0.0.1@evil.example/`), as a subdomain
(`http://127.0.0.1.evil.example/`), as a label (`http://localhost.evil.example/`),
and in a path or query where it decides nothing. The check runs before every
request rather than only in `availability`, because a provider constructed
directly — by a test, an embedder, or the FFI — never passes through a
diagnosis.

No credential is sent to a local server unless one is configured through the
local variable, which is deliberately not the hosted one. The credential for a
hosted OpenAI-compatible service travels as a header through the same transport
as every other provider's, so it stays out of argument vectors and out of the
audit log (ADR-0016).

## Revisit when

- A local server becomes common enough that its native API is worth speaking
  directly, or the compatibility endpoints diverge enough that one format stops
  reaching all of them.
- Somebody needs a model on another machine they control, often enough that
  `--provider openai` with a base URL is the wrong shape for it.
- Local models get good enough at single-shot structured output that the
  caveat above stops being true, at which point `local` deserves to be the
  default rather than an option.
