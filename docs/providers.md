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
| `anthropic` | Anthropic | `ANTHROPIC_API_KEY` |
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

Speaks Anthropic's Messages API. The model is `claude-sonnet-5` and is not
configurable from the environment yet — unlike the other two, which have to be,
because a base URL somebody else chose implies a model somebody else named.

## `openai` — OpenAI, and everything that copied it

| | |
|---|---|
| `OPENAI_API_KEY` | required |
| `OPENAI_BASE_URL` | default `https://api.openai.com/v1` |
| `OPENAI_MODEL` | default `gpt-5` |
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

Whatever it points at, this provider sends your intent off the machine. That is
the whole of the difference between it and `local`, which uses the same wire
format ([ADR-0019](architecture/decisions/0019-openai-compatible-and-a-local-model.md)).

## What is not tested here

CI never makes a live model call
([ADR-0008](architecture/decisions/0008-agent-provider-abstraction.md)), so
every provider's request building, response parsing and error mapping is tested
as pure functions against recorded replies, and the part that is not tested is
one shared module that hands a string to `curl`. That applies to `local` too:
nothing in CI runs a model server either.
