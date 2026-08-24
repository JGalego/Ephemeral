//! The JavaScript interpreter Ephemeral ships.
//!
//! ## Why this exists
//!
//! A phone can run a WebAssembly module and cannot compile one. So an
//! application a model wrote ten seconds ago can only run on the device it was
//! written on if it is a *script*, and something already on the device is a
//! module that runs scripts ([ADR-0021], [ADR-0022]).
//!
//! This is that module. It is handed the path of a script, reads it, and runs
//! it. Nothing about being an interpreter buys it anything: it gets the same
//! preopens, the same fuel, the same memory ceiling and the same absence of
//! sockets as any other application, and the script's own directory is mounted
//! read-only so a running application cannot edit itself between runs.
//!
//! ## Why JavaScript, and why Boa
//!
//! JavaScript because it is the language every model writes best, and because
//! its whole runtime is small enough to be a file somebody downloads with an
//! application.
//!
//! Boa because it is written in Rust and interprets. Every mature alternative
//! is either C — needing a toolchain targeting WebAssembly that a phone does
//! not have — or a just-in-time compiler, and iOS does not allow an application
//! to generate and execute machine code. That is the same constraint that chose
//! wasmi over wasmtime, one level down.
//!
//! ## What a script gets
//!
//! Small, and deliberately so. Everything here is something a generated
//! application genuinely needs, and nothing here widens what it may do:
//!
//! | | |
//! |---|---|
//! | `console.log(…)`, `console.error(…)` | writing, which is how an application answers |
//! | `Ephemeral.args` | what somebody filled into the form |
//! | `Ephemeral.read(path)` | a file, if one was granted |
//! | `Ephemeral.write(path, text)` | a file, if writing was granted |
//! | `Ephemeral.get(url)`, `Ephemeral.post(url, body)` | the network, if a destination was allowed |
//!
//! There is no `fetch`, no `require`, no `process`, no timers and no way to
//! spawn anything, because there is nothing underneath them: this module
//! imports the host functions for exactly the five things above and WASI for
//! the rest. A script calling anything else gets a `ReferenceError`, which is
//! an honest answer.
//!
//! [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md
//! [ADR-0022]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0022-how-an-interpreter-reaches-a-device.md

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsResult, JsString, JsValue, NativeFunction, Source};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some((script, given)) = arguments.split_first() else {
        eprintln!("this interpreter needs the path of a script to run");
        std::process::exit(1);
    };

    match run(script, given) {
        Ok(()) => {}
        Err(said) => {
            eprintln!("{said}");
            std::process::exit(1);
        }
    }
}

/// Reads a script and runs it under everything below.
fn run(script: &str, given: &[String]) -> Result<(), String> {
    let source = std::fs::read_to_string(script)
        .map_err(|error| format!("{script} could not be read: {error}"))?;

    let mut context = Context::default();
    furnish(&mut context, given)?;

    match context.eval(Source::from_bytes(source.as_bytes())) {
        Ok(_) => Ok(()),
        // The script's own error, in the words it wrote, with its stack. A
        // person looking at "TypeError: x is not a function" can act on it; a
        // person looking at "the interpreter returned 1" cannot.
        Err(error) => Err(said(&error, &mut context)),
    }
}

/// What a script can see.
fn furnish(context: &mut Context, given: &[String]) -> Result<(), String> {
    fn failed(what: &'static str) -> impl Fn(JsError) -> String {
        move |error| format!("{what} could not be prepared: {error}")
    }

    let console = ObjectInitializer::new(context)
        .function(NativeFunction::from_fn_ptr(log), JsString::from("log"), 0)
        .function(NativeFunction::from_fn_ptr(warn), JsString::from("warn"), 0)
        .function(
            NativeFunction::from_fn_ptr(warn),
            JsString::from("error"),
            0,
        )
        .build();
    context
        .register_global_property(JsString::from("console"), console, Attribute::all())
        .map_err(failed("console"))?;

    let arguments = boa_engine::object::builtins::JsArray::from_iter(
        given
            .iter()
            .map(|argument| JsValue::from(JsString::from(argument.as_str()))),
        context,
    );

    let ephemeral = ObjectInitializer::new(context)
        .property(JsString::from("args"), arguments, Attribute::all())
        .function(NativeFunction::from_fn_ptr(read), JsString::from("read"), 1)
        .function(
            NativeFunction::from_fn_ptr(write),
            JsString::from("write"),
            2,
        )
        .function(NativeFunction::from_fn_ptr(get), JsString::from("get"), 1)
        .function(NativeFunction::from_fn_ptr(post), JsString::from("post"), 2)
        .build();
    context
        .register_global_property(JsString::from("Ephemeral"), ephemeral, Attribute::all())
        .map_err(failed("Ephemeral"))?;

    Ok(())
}

/// `console.log(…)` — what an application says.
fn log(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    println!("{}", joined(arguments, context)?);
    Ok(JsValue::undefined())
}

/// `console.warn(…)` and `console.error(…)`, which are the same stream.
fn warn(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    eprintln!("{}", joined(arguments, context)?);
    Ok(JsValue::undefined())
}

/// `Ephemeral.read(path)` — a file, if one was granted.
///
/// The failure names the sandbox, because that is the likeliest reason and the
/// one somebody can do something about. "No such file or directory" for a file
/// a person can see in their own folder is a confusing way to learn that an
/// application was not allowed to read it.
fn read(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = text(arguments.first(), context)?;

    match std::fs::read_to_string(&path) {
        Ok(contents) => Ok(JsValue::from(JsString::from(contents))),
        Err(error) => Err(refused(&format!(
            "{path} could not be read: {error}. If it is there, this application \
             may not have been allowed to read it."
        ))),
    }
}

/// `Ephemeral.write(path, text)` — a file, if writing was granted.
fn write(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let path = text(arguments.first(), context)?;
    let contents = text(arguments.get(1), context)?;

    match std::fs::write(&path, contents) {
        Ok(()) => Ok(JsValue::undefined()),
        Err(error) => Err(refused(&format!(
            "{path} could not be written: {error}. If the folder is there, this \
             application may not have been allowed to change it."
        ))),
    }
}

/// `Ephemeral.get(url)`.
fn get(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = text(arguments.first(), context)?;
    answered(ephemeral_app::get(&url), context)
}

/// `Ephemeral.post(url, body)`.
fn post(_this: &JsValue, arguments: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = text(arguments.first(), context)?;
    let body = text(arguments.get(1), context)?;
    answered(ephemeral_app::post(&url, &body), context)
}

/// Turns an answer into `{status, body, ok}`, and a refusal into a thrown
/// `Error` carrying Ephemeral's own words.
fn answered(
    outcome: Result<ephemeral_app::Answer, ephemeral_app::Refused>,
    context: &mut Context,
) -> JsResult<JsValue> {
    let answer = outcome.map_err(|refusal| refused(refusal.said()))?;

    let object = ObjectInitializer::new(context)
        .property(
            JsString::from("status"),
            JsValue::from(answer.status),
            Attribute::all(),
        )
        .property(
            JsString::from("body"),
            JsValue::from(JsString::from(answer.body.as_str())),
            Attribute::all(),
        )
        .property(
            JsString::from("ok"),
            JsValue::from(answer.ok()),
            Attribute::all(),
        )
        .build();

    Ok(JsValue::from(object))
}

/// One argument, as a string.
fn text(value: Option<&JsValue>, context: &mut Context) -> JsResult<String> {
    let value = value.cloned().unwrap_or(JsValue::undefined());
    Ok(value.to_string(context)?.to_std_string_escaped())
}

/// Every argument, joined the way `console.log` joins them.
fn joined(arguments: &[JsValue], context: &mut Context) -> JsResult<String> {
    let mut written = Vec::with_capacity(arguments.len());
    for argument in arguments {
        written.push(argument.to_string(context)?.to_std_string_escaped());
    }
    Ok(written.join(" "))
}

/// An error a script can catch, carrying a sentence meant for a person.
fn refused(said: &str) -> JsError {
    JsError::from_opaque(JsValue::from(JsString::from(said)))
}

/// What went wrong, as a script would have printed it.
fn said(error: &JsError, context: &mut Context) -> String {
    error.to_opaque(context).to_string(context).map_or_else(
        |_| error.to_string(),
        |written| written.to_std_string_escaped(),
    )
}
