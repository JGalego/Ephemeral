# 23. A confined application reaches the network through its host

Date: 2026-08-24

## Status

Accepted.

## Context

[ADR-0021](0021-webassembly-is-the-runtime-a-phone-can-have.md) made a phone
able to run an application, and made one thing permanently untrue of it:
**a WebAssembly application under this runtime has no sockets.** Not "network
denied" — no socket API for the module to import at all. That was written down
as a virtue, and it is one: it is why showing somebody a user interface costs no
network permission, and why the sandbox's strongest claim needs no enforcement
layer to be true.

It also meant a whole class of application could not exist. Two people, two
devices, one conversation: there is nothing in the sandbox that can carry a
byte from one to the other. The first version of a messaging application written
for this runtime passed messages through a folder both parties were granted,
which works exactly as long as both parties share a filesystem — which is to say,
not at all for the case anybody actually means.

Worse, the permission model already described the thing the runtime could not
do. `net:api.example.com` is a grant a person can make, the container runtime
honours it, and the interface offers it. The WebAssembly runtime refused to
start any application holding one, with a message pointing at Docker. A
capability that exists everywhere except on the device most people are holding
is not a capability.

The constraint that produced no-sockets has not changed. An interpreter that
opened its own connections would need an HTTP client compiled into Ephemeral,
its own TLS decisions, its own proxy handling and its own certificate policy —
on iOS, on Android, and on three desktops. Every one of those is a decision the
platform has already made better than we would.

## Decision

**An application does not connect to anything. It describes a request, and
something outside the sandbox makes it.**

That something is a `Reach`, supplied by whoever is running the application:

| Where | What carries it |
|---|---|
| Desktop, terminal | `curl`, spawned — as for a model provider ([ADR-0016](0016-real-providers-live-in-their-own-crates.md)) |
| Android, iOS | the host callback that already reaches a model ([ADR-0017](0017-mobile-generates-through-a-host-transport.md)) |
| Anything else | whatever the embedder supplies, or nothing |

`ephemeral-runtime` implements none of them. It has no HTTP client, opens no
socket and resolves no name, which is what keeps *"the sandbox cannot reach the
network"* a fact about the code rather than a claim about its behaviour.

### What the application sees

Two imports, in a module named `ephemeral`, linked **only** when a person has
granted egress *and* something was supplied to carry it:

```
ephemeral.send(request_ptr: i32, request_len: i32) -> i32   // the answer's size
ephemeral.recv(into_ptr: i32, room: i32)           -> i32   // bytes copied
```

A request is `{"method":"GET"|"POST","url":"…","body":"…"}` and nothing else.
An answer is always a readable document, refusals included:
`{"status":200,"body":"…"}` or `{"status":0,"error":"…"}`.

**No headers.** An application that could set a header on a request the host
performs is an application that can attach a credential it was never shown to a
destination of its choosing. Nothing a generated application legitimately does
needs one.

Two calls rather than one because a host call cannot return a variable-length
value. The module is told the size, makes room, and asks for it — rather than
guessing a buffer and being handed a silent truncation, which for a message from
another person is the worst available failure.

### Where the decision is made

**In the runtime, against the grant, before the host is asked.** Not by the
host.

A phone application deciding for itself which destinations are allowed would be
a second copy of the permission model, in another language, on another release
cycle, and the copy that drifts is the one nobody is looking at. A `Reach` is
trusted to make the request it is given. It is not trusted to decide whether to.

### Two bounds that are not the existing ones

**Requests are counted.** Fuel meters instructions, and a host call spends
almost none — the waiting happens outside the interpreter. Without a separate
budget, "it cannot run forever" would be true of the module and false of the
run: an application could loop on requests for as long as the other end kept
answering, having spent nearly nothing. Sixty-four per run.

**Bodies are capped**, in both directions, at a megabyte. The store's memory
limit bounds what a module allocates for itself and says nothing about what the
host holds on its behalf.

### An application with no grant does not start

The imports are not linked, so a module that names them has nothing to bind to
and never executes an instruction. There is no version of this where the
application runs and its requests quietly fail. The refusal is named in the
words a person granted it in — *"it needs to reach a service over the network,
and has not been allowed to reach anything"* — rather than in the interpreter's.

## Consequences

**Two devices can hold a conversation.** Demonstrated: two Ephemeral installs
with separate ledgers and separate audit logs, sharing no filesystem, each
granted one address and nothing else, exchanging messages through a relay
neither of them controls. Every other destination is refused before anything is
asked to carry it, including a different port on the same host.

**The refusal in `WasmRuntime::capabilities` changed meaning.** It used to say
"this runtime has no network, use Docker". It now fires only when nothing was
supplied to carry a request, and says that instead. A caller that passes no
`Reach` gets exactly the old behaviour, which is the honest one for a caller
that has no network to lend.

**A guest needs `unsafe` to declare the imports.** That is inherent to a C ABI
in WebAssembly and is the same thing every WASI program does. A thin safe
wrapper crate for generated applications would remove it from the code a model
writes; nothing depends on that yet.

**The phone reports the status too.** `EphemeralHttpSend` gained an
`int32_t *status` the host writes what it got into, so an application sees the
same number on a handset as in a terminal. A host with none to give leaves it
zero, and zero reaches the application as zero — an invented `200` is a number
it might branch on. Java has no out-parameters, so the Kotlin side takes a
one-element `IntArray`, which is the shape the platform's own APIs use.

**This widens what a generated application can do**, and that deserves saying
plainly. Before this, the strongest thing Ephemeral could promise about a
WebAssembly application was that it could not talk to anyone, and that promise
was free — it needed no enforcement because there was nothing to enforce.
Now it costs a check, and the check is code that can be wrong. What has not
changed: nothing reaches anywhere unless a person allowed that destination by
name, the destination is checked in one place, and a redirect is not followed —
because a redirect is a destination nobody was asked about.
