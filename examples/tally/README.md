# The reference WebAssembly application

What a generated application looks like when it runs on a phone.

Everything else that exercises Ephemeral's WebAssembly runtime assembles a
module out of WebAssembly text. That proves the sandbox holds; it does not prove
somebody could *write* something for it. This is a real program — ordinary Rust,
no dependencies, 69 KB compiled — run through exactly the sandbox any other
application gets.

## What it shows

**Tier two.** It declares what it takes, and a client draws a form from that
declaration rather than asking somebody to type a command line. The arguments
below are what that form composes.

```
--file <path>      the CSV to count
--no-headers       count the first line as a row rather than a header
--format html      write a page instead of a line
```

**Tier one.** With `--format html` it writes a page, and the host renders it. A
WebAssembly application has no socket and cannot be a server, which is exactly
why showing somebody a user interface costs no network permission at all — see
[ADR-0021](../../docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md).

## Running it

```
rustup target add wasm32-wasip1
cargo build --release --target wasm32-wasip1
```

Then make it into a package — an `ephemeral.yaml` beside a `source/` holding
`program.wasm` — whose runtime block says:

```yaml
runtime:
  type: wasm
  program: program.wasm
  interface: job          # or `web`, to write a page instead of a line
  entrypoint: ["--file", "/data/files.csv"]
```

and install it:

```
ephemeral install <the package> --accept
ephemeral generate <id>          # checks the module; no model, no Docker
ephemeral run <id>
```

`generate` is the same command a container application uses, and it does the
same job: whatever remains between a recipe and something runnable. For a
container that is building an image. For this it is checking that the module
loads, has an entry point, and asks for nothing it was not given — which is
what stops an application installing cleanly and then failing the moment
somebody presses Run.

`crates/ephemeral-runtime/tests/reference_wasm.rs` runs the module directly, and
`ephemeral-engine`'s tests cover the install-and-adopt path, so both are checked
rather than described.

## What it cannot do

It opens no socket, has no dependencies, and reads only the file it was given.
Not because this code is careful — if it asked for anything else the module
would not start, because there would be nothing for the request to bind to.

Point it at a file it was not granted and it exits 1 with a sentence naming the
likeliest reason. That message is the one a person actually sees when a grant is
missing, which is why it says "this application may not have been allowed to
read it" rather than repeating the operating system's "no such file".

## What it is not

It is not what generation produces. Nothing generates WebAssembly yet — an
application must be a compiled module like this one, or a script whose
interpreter is installed, and no interpreter ships. See
[ADR-0022](../../docs/architecture/decisions/0022-how-an-interpreter-reaches-a-device.md),
which is proposed rather than decided.
