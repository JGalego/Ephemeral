# The reference application for a network

Two people, two devices, one conversation — and nothing shared between them.

[`tally`](../tally) shows what a generated application looks like when it runs
on a phone. This shows what one looks like when it needs to reach somebody:
about a hundred and eighty lines of ordinary Rust, one dependency, **no
`unsafe`**, compiled to `wasm32-wasip1`.

## What it shows

**A confined application has no socket, and reaches the network anyway.** There
is no socket in this runtime and there will not be
([ADR-0021](../../docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md)).
What this application does is *describe* a request; Ephemeral checks it against
what a person allowed and asks whoever is running the application to make it —
`curl` in a terminal, the platform's own HTTPS on a phone
([ADR-0023](../../docs/architecture/decisions/0023-a-confined-application-reaches-the-network-through-its-host.md)).

The whole of the network, in this application, is four lines:

```rust
let answer = match &asked.send {
    Some(message) => ephemeral_app::post(&url, &line),
    None => ephemeral_app::get(&url),
}
.map_err(|refused| refused.said().to_owned())?;
```

`Cargo.toml` says `unsafe_code = "forbid"`, and it compiles. The one `extern`
block there is lives in [`ephemeral-app`](../../crates/ephemeral-app), reviewed
once, rather than in every application a model writes.

**A refusal is a sentence, not a code.** Point it somewhere it was not granted
and `Refused::said()` carries Ephemeral's own words, which are meant to be shown
to a person:

```
this application was not allowed to reach api.openai.com:443.
It has network access to 127.0.0.1:8787.
```

**A status is a status.** The relay's `503` reaches the application as `503`,
on a desktop and on a handset, so it can say *"the room answered 503 rather than
carrying that"* rather than rendering an error page as a conversation.

## The protocol, such as it is

A room is a URL. A message is a line — a name, a tab, what they said. `POST`
appends one; `GET` reads them all back. Whoever holds the room holds the
conversation, which is worth being plain about: this is a demonstration of a
capability, not a secure messenger. There is no end-to-end encryption here and
nothing pretends otherwise.

```
--as Alice --relay https://rendezvous.example.com --room garden --send "hello"
--as Bob   --relay https://rendezvous.example.com --room garden --read
--as Bob   --relay https://rendezvous.example.com --room garden --read --format html
```

With `--format html` it writes a page and the host renders it — tier one, and no
network permission of its own, because a page is not a server.

## Running it

```
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
```

Getting it into Ephemeral is the same hand-assembly [`tally`'s README](../tally/README.md#running-it)
describes, with one addition: it needs somewhere to reach, and needs to say so.

```json
"permissions": {
  "network": { "outbound": true, "allowed_hosts": ["rendezvous.example.com"] }
}
```

Then, and this is the part that matters:

```
ephemeral grant <id> net:rendezvous.example.com
ephemeral grant ephemeral network
```

Both are needed and they are different questions. The first is *this application
may reach that host*. The second is *Ephemeral itself may carry a request on an
application's behalf at all* — the two-tier model
([ADR-0003](../../docs/architecture/decisions/0003-two-tier-permission-model.md)),
which is why a grant can sit in the ledger doing nothing until you allow the
thing that would act on it.

Without the first, the application **does not start**: the host functions are
not linked, so the module has nothing to bind to, and Ephemeral says so in the
words the grant is made in rather than the interpreter's.

## What it cannot do

Reach anywhere else. Set a header. Open a socket. Read a file it was not given —
it is granted no filesystem at all, which the run banner says outright:

```
Has network access to 127.0.0.1:8787
```

and nothing else on the line.
