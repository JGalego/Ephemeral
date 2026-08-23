# Providers

A provider is where Ephemeral gets the code from. It is chosen per run:

```console
$ ephemeral generate <app> --provider local
```

There are four, and the difference that matters between them is not quality.
It is **where what you asked for goes**.

| Provider | Where the intent goes | What it needs |
|---|---|---|
| `mock` | nowhere | nothing |
| `local` | a model server on this machine | a model server on this machine |
| `anthropic` | Anthropic, or whatever `ANTHROPIC_BASE_URL` points at | `ANTHROPIC_API_KEY` |
| `openai` | OpenAI, or whatever `OPENAI_BASE_URL` points at | `OPENAI_API_KEY` |

`mock` is the default, because it is the one that works with nothing installed
and no account anywhere.

## What every provider has in common

The prompts, the reply parsing and the validation are shared. A provider
contributes a request envelope and knows where the text sits in a reply; it does
not get its own instructions, so two providers cannot drift into building subtly
different applications from the same sentence.

Nothing a model returns is trusted by any of them. A reply that does not parse
is an error rather than a best-effort guess, a plan that asks for a capability
without explaining why is refused, and a file path that escapes the application
is refused rather than normalised. None of that depends on the model
cooperating.

The credential, where there is one, travels in a header to `curl` through a
configuration document on stdin — never in an argument vector, which is why the
command Ephemeral ran is safe to record in the audit log
([ADR-0016](architecture/decisions/0016-real-providers-live-in-their-own-crates.md)).

## `mock` — no model at all

A deterministic provider that returns a real, working CSV comparator, including
its tests. It exists so that the build-and-repair loop can be exercised in CI
without a credential, a network connection or a bill — and so that somebody
trying Ephemeral for the first time sees it work end to end before deciding
whether to point it at a model.

## `local` — nothing leaves the machine

```console
$ ollama serve                       # or llama.cpp, LM Studio, vLLM
$ ollama pull qwen2.5-coder
$ ephemeral generate <app> --provider local
```

| | |
|---|---|
| `EPHEMERAL_LOCAL_BASE_URL` | default `http://127.0.0.1:11434/v1` — Ollama's |
| `EPHEMERAL_LOCAL_MODEL` | default `qwen2.5-coder` |
| `EPHEMERAL_LOCAL_API_KEY` | only for a local server started with one |

**The endpoint must be on this machine.** Not by convention — it is checked
before every request, and an endpoint that is not a loopback address is refused
by name. That includes the ones written to look local and resolve elsewhere:
`http://127.0.0.1@elsewhere.example/`, where the loopback address is a username,
and `http://127.0.0.1.elsewhere.example/`, where it is a subdomain. If you want
a model server on another machine, use `--provider openai` with
`OPENAI_BASE_URL`, which is honest about what it is.

The credential variable is deliberately not `OPENAI_API_KEY`. A hosted
credential sitting in your environment is never handed to a process on your
machine merely because both speak the same protocol.

**Two honest caveats.** Generation asks a model to return a whole application as
one valid JSON object, and models small enough to run comfortably on a laptop
fail at that more often than hosted ones do — the loop's repair round absorbs
some of it, and the rest you see as a refusal to act on an unreadable reply. And
a local model is a *more private* model, not a more trustworthy one: its output
is validated identically, because privacy is not integrity.

## `anthropic` — a hosted model

| | |
|---|---|
| `ANTHROPIC_API_KEY` | required |
| `ANTHROPIC_BASE_URL` | default `https://api.anthropic.com` |
| `ANTHROPIC_MODEL` | default `claude-sonnet-5` |
| `ANTHROPIC_MAX_TOKENS` | default `32000` |

Speaks Anthropic's Messages API — Anthropic's own endpoint by default, and any
gateway or proxy that speaks it otherwise. The model was fixed here for a while,
on the reasoning that Anthropic's endpoint implies Anthropic's model names. That
stopped being true the moment the endpoint could be pointed elsewhere.

## `openai` — OpenAI, and everything that copied it

| | |
|---|---|
| `OPENAI_API_KEY` | required |
| `OPENAI_BASE_URL` | default `https://api.openai.com/v1` |
| `OPENAI_MODEL` | default `gpt-5` |
| `OPENAI_MAX_TOKENS` | default `32000` |
| `OPENAI_TOKEN_CEILING_FIELD` | `max_completion_tokens` (default) or `max_tokens` |

The chat completions format is the closest thing generation has to a common
language, so this one provider reaches OpenAI, a company gateway, a proxy, or a
hosted service that speaks the same API:

```console
$ export OPENAI_BASE_URL=https://models.example.com/v1
$ export OPENAI_MODEL=some-model-they-host
$ ephemeral generate <app> --provider openai
```

`OPENAI_TOKEN_CEILING_FIELD` is there for a service that copied this format
before OpenAI renamed `max_tokens`: every request bounds its reply, and a bound
sent under a name the service does not read is no bound at all. Set to anything
other than those two names, it is refused rather than quietly defaulted past.

### Groq, and anything else that copied the format

Groq speaks this API, so it needs no code and no new provider — three variables
and it is a different model:

```console
$ export OPENAI_BASE_URL=https://api.groq.com/openai/v1
$ export OPENAI_API_KEY=gsk_...
$ export OPENAI_MODEL=llama-3.3-70b-versatile
$ ephemeral generate <app> --provider openai
```

The key comes from [console.groq.com](https://console.groq.com) → API Keys.
Groq reads `max_completion_tokens`, which is the default, so
`OPENAI_TOKEN_CEILING_FIELD` can be left alone. Its catalogue changes faster
than a document can track — if a model id has been retired the request fails
with the service's own message saying so, which is the right place to find out.

The same three variables reach Together, Fireworks, OpenRouter, a company
gateway, or a colleague's vLLM. None of them is more trusted than the others:
what a model returns is validated identically no matter who served it.

Whatever it points at, this provider sends your intent off the machine. That is
the whole of the difference between it and `local`, which uses the same wire
format ([ADR-0019](architecture/decisions/0019-openai-compatible-and-a-local-model.md)).

## Checking before you spend

```console
$ ephemeral models --provider openai
```

One command for two questions, because they have one answer: it asks the
service what models it has, using the endpoint and credential generation would
use. A wrong key, a base URL pointing at nothing, or a model that has been
retired all surface here rather than halfway through a generation somebody is
paying for — and the refusal is the service's own words, because "Invalid API
Key" from the vendor beats anything Ephemeral could invent.

It also prints how large a reply each model will accept, where the service
publishes that. This is the setting most likely to be wrong and the hardest to
guess:

```text
  openai/gpt-oss-120b     GPT OSS 120B      up to 65536 tokens
  qwen/qwen3.6-27b        Qwen/Qwen3.6-27B  up to 16384 tokens
  allam-2-7b              ALLaM-2-7b        up to 4096 tokens
```

Ephemeral asks for a 32,000-token reply by default, because a whole application
plus the model's own reasoning has to arrive in one piece. Point it at a model
that holds less and the service refuses outright, complaining about a field
nobody typed. `OPENAI_MAX_TOKENS` — and its equivalents for the other two
providers — is what makes those models usable. It is a real trade: a reply that
runs out mid-way is half a JSON object, which parses as nothing.

Models that cannot emit text are left out of the listing. A service that says a
model produces speech or a transcription is telling you it cannot write an
application, and offering it would be offering a choice that fails later for a
reason nobody could predict from the name.

## On a phone

Every provider but `local` is reachable from Android and iOS, chosen in the app
under **Model**: a service, a base URL, a model name, and a key from the
platform's secure store. Groq on a handset is the same three settings it is
here. **Check connection** on that screen is `ephemeral models` — it reports
what the service said, and fills the model box from what came back, so a name
does not have to be typed from memory.

`local` is absent, and not by oversight. It exists to keep an intent on the
machine that generated it, and it refuses any endpoint that is not loopback —
which on a phone means a model server running on that phone. Anything else
somebody means by "local" is another machine, and that is `openai` with a base
URL, which is what it honestly is.

A phone plans and writes an application. It cannot build, run or repair one:
that needs a container runtime, which is why a phone is a control plane and not
a second desktop
([ADR-0007](architecture/decisions/0007-mobile-control-plane.md)).

For a long time a phone could reach Anthropic and nothing else — not through
configuration, but because the C ABI took a bare API key and left the host to
wrap it in Anthropic's headers. See
[ADR-0020](architecture/decisions/0020-the-host-chooses-the-provider.md) for why
that was wrong and what replaced it.

## What is not tested here

CI never makes a live model call
([ADR-0008](architecture/decisions/0008-agent-provider-abstraction.md)), so
every provider's request building, response parsing and error mapping is tested
as pure functions against recorded replies, and the part that is not tested is
one shared module that hands a string to `curl`. That applies to `local` too:
nothing in CI runs a model server either.

What is not automated is *looking*. `scripts/many-apps` asks a real model for
eight different applications, builds and runs each one, and writes a report —
including what each asked to be allowed and what it printed when handed a real
file. It is dispatch-only in CI (`.github/workflows/many-apps.yml`) and free to
run locally against the mock:

```console
$ scripts/many-apps                       # the mock: one fixed app, eight times
$ scripts/many-apps --provider openai     # whatever OPENAI_BASE_URL points at
```

Run against the mock it reports one application in eight doing the job, which is
correct and is the point: the mock returns a CSV comparator whatever you ask it
for, and seven of those eight sentences were not about CSVs.
