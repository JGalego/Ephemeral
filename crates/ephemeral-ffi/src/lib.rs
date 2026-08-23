//! # Ephemeral's C ABI
//!
//! What iOS and Android link against, so a phone runs the same manifests, the
//! same lifecycle machine, the same permission ledger and the same audit record
//! as the desktop. There is no second implementation of any of that, which is
//! the only way the promises stay the same on both.
//!
//! ## What a phone can and cannot do
//!
//! It can **create** an application, **plan** it, and **generate** it: that is a
//! description, an HTTPS request, and some parsing. It can also **run** one, as
//! WebAssembly, in this process — see [`ephemeral_run`] and [ADR-0021].
//!
//! It cannot **build** or **repair**. Both mean a container: building is a
//! Dockerfile and repairing is building again with the failure in hand, and a
//! phone has no daemon to do either with. An application generated here for a
//! container is therefore real and versioned, with its source written and its
//! requested permissions recorded, and its lifecycle stops at "generated, not
//! built" — a state the machine already models. A machine that can build
//! finishes the job.
//!
//! [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md
//!
//! ## The host supplies the network and the credential
//!
//! Nothing here opens a socket or reads an environment variable. The host
//! passes in two function pointers and a context; Ephemeral calls them when it
//! needs an HTTPS round trip. That keeps TLS, certificate pinning, and
//! background-transfer policy where the platform already owns them — and it is
//! what makes generation possible on iOS at all, where spawning `curl` is not
//! an option.
//!
//! The credential is passed in the same way, from whatever secure store the
//! platform provides. `ANTHROPIC_API_KEY` is a desktop convention and is never
//! read here.
//!
//! ## Every function in this crate
//!
//! Returns either an owned C string the caller frees with
//! [`ephemeral_string_free`], or a status code. Nothing panics across the
//! boundary: an unwind through a C frame is undefined behaviour, so every entry
//! point catches one and turns it into a failure the host can read with
//! [`ephemeral_last_error`].
//!
//! [ADR-0007]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0007-mobile-control-plane.md

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use ephemeral_agent::AgentProvider;
use ephemeral_agent::transport::{HttpRequest, Transport};
use ephemeral_core::{
    Actor, AppId,
    lifecycle::TransitionRequest,
    storage::{AppStore as _, Workspace},
};
use serde_json::Value;

mod model;
mod run;

pub use model::{Choice, DEFAULT_PROVIDER, Described, catalogue};

/// Everything went as asked.
pub const EPHEMERAL_OK: c_int = 0;

/// Something failed; [`ephemeral_last_error`] says what.
pub const EPHEMERAL_ERROR: c_int = -1;

/// A handle was null or not one this library produced.
pub const EPHEMERAL_BAD_HANDLE: c_int = -2;

/// Sends one HTTPS request, supplied by the host.
///
/// Returns a newly allocated, NUL-terminated JSON response, or null on failure.
/// Ephemeral copies the response immediately and then hands the pointer back to
/// the matching free function, so the host owns the allocation throughout.
///
/// Every argument is a NUL-terminated UTF-8 string that is only valid for the
/// duration of the call.
///
/// `headers_json` is the complete header set the provider composed, in order,
/// as `[{"name":"…","value":"…"}, …]`. The host sets exactly these and adds
/// nothing: the credential is one of them, and so is whatever else the service
/// requires. It used to be a single `api_key` the host wrapped in Anthropic's
/// headers, which quietly made the ABI belong to one vendor — a phone could not
/// be pointed anywhere else no matter what anybody configured.
pub type EphemeralHttpSend = extern "C" fn(
    context: *mut c_void,
    endpoint: *const c_char,
    headers_json: *const c_char,
    request_json: *const c_char,
) -> *mut c_char;

/// Releases a response previously returned by an [`EphemeralHttpSend`].
pub type EphemeralHttpFree = extern "C" fn(context: *mut c_void, response: *mut c_char);

/// A transport that calls back into the host to make its requests.
///
/// This is the seam that makes generating on a phone possible. On iOS it is
/// `URLSession`; on Android, whatever the app already uses. Ephemeral does not
/// bring an HTTP stack, does not open a socket, and does not make a policy
/// decision about TLS — the platform keeps all of that.
struct HostTransport {
    context: *mut c_void,
    send: EphemeralHttpSend,
    free: EphemeralHttpFree,
}

// SAFETY-adjacent reasoning, stated rather than assumed: `context` is an opaque
// pointer the host owns. Ephemeral never dereferences it — it only hands it
// back to the host's own functions. The host is responsible for those being
// callable from whichever thread it drives Ephemeral on, which is the same
// contract every C callback API has.
#[allow(unsafe_code)]
// SAFETY: the pointer is never dereferenced here; it is opaque to this crate.
unsafe impl Send for HostTransport {}
#[allow(unsafe_code)]
// SAFETY: as above — no dereference, therefore no aliasing hazard introduced.
unsafe impl Sync for HostTransport {}

impl Transport for HostTransport {
    fn send(&self, request: &HttpRequest<'_>) -> Result<Value, ephemeral_agent::AgentError> {
        // The whole header set, exactly as the provider composed it. Nothing
        // here inspects it and nothing on the other side adds to it: which
        // headers a service wants is the provider's knowledge, and a transport
        // that knew any of them would be a transport that belongs to one
        // vendor. That is precisely what this was, and it is why a phone could
        // only ever reach Anthropic.
        let headers: Vec<Header<'_>> = request
            .headers
            .iter()
            .map(|(name, value)| Header { name, value })
            .collect();

        let failure = |reason: String| ephemeral_agent::AgentError::Failed {
            provider: "host".to_owned(),
            reason,
        };

        let written = serde_json::to_string(&headers)
            .map_err(|error| failure(format!("the headers could not be written: {error}")))?;

        let endpoint = CString::new(request.endpoint)
            .map_err(|_| failure("the endpoint is not a C string".to_owned()))?;
        let headers =
            CString::new(written).map_err(|_| failure("a header is not a C string".to_owned()))?;
        let body = CString::new(request.body.to_string())
            .map_err(|_| failure("the request body is not a C string".to_owned()))?;

        let reply = (self.send)(
            self.context,
            endpoint.as_ptr(),
            headers.as_ptr(),
            body.as_ptr(),
        );

        if reply.is_null() {
            return Err(failure(
                "the host's HTTPS transport reported a failure".to_owned(),
            ));
        }

        // Copy before releasing: the host owns the allocation, and holding a
        // borrow of it past the free would be a use-after-free.
        #[allow(unsafe_code)]
        // SAFETY: non-null, and by the documented contract of `EphemeralHttpSend`
        // it is a NUL-terminated string that stays valid until `free` is called.
        let copied = unsafe { CStr::from_ptr(reply) }
            .to_string_lossy()
            .into_owned();
        (self.free)(self.context, reply);

        serde_json::from_str(&copied).map_err(|error| ephemeral_agent::AgentError::Unreadable {
            provider: "host".to_owned(),
            reason: format!("the API's reply was not JSON: {error}"),
            raw: copied,
        })
    }
}

/// One header, as it crosses the boundary.
///
/// Named fields rather than a two-element array, because a host reading
/// `header["name"]` cannot get it the wrong way round and a host reading
/// `header[0]` can.
#[derive(serde::Serialize)]
struct Header<'a> {
    name: &'a str,
    value: &'a str,
}

/// One open Ephemeral, as the host sees it.
///
/// Opaque on purpose: the host holds a pointer and nothing else, so the layout
/// of everything inside can change without breaking a build somebody shipped.
pub struct Ephemeral {
    home: PathBuf,

    /// How to reach the host's HTTPS stack. Held as its parts rather than as a
    /// built transport, because a provider owns its transport and a provider is
    /// now built fresh for every call.
    context: *mut c_void,
    send: EphemeralHttpSend,
    free: EphemeralHttpFree,

    /// Which service, and how it is configured.
    choice: Mutex<model::Choice>,

    /// The credential, from the platform's secure store. Never read from an
    /// environment variable here: that is a desktop convention, and a phone has
    /// no environment to read.
    credential: Mutex<Option<String>>,

    /// Why the last call failed, for a host that wants to show it.
    last_error: Mutex<Option<String>>,
}

impl Ephemeral {
    /// Opens the workspace fresh for each operation.
    ///
    /// Mobile applications are suspended and resumed without warning, and a
    /// long-lived open handle to files the OS may have moved underneath us is a
    /// good way to corrupt them. Re-opening is cheap and is what the CLI does.
    fn workspace(&self) -> Result<Workspace, String> {
        Workspace::open(&self.home)
            .map_err(|error| format!("could not open Ephemeral's files: {error}"))
    }

    fn remember(&self, reason: &str) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(reason.to_owned());
        }
    }

    /// The provider to use for this call.
    ///
    /// Built per call rather than held, which is what makes changing the
    /// provider or the credential a matter of writing down a different choice.
    /// The version of this that held one built provider had to swap it out
    /// through a placeholder transport whenever a credential arrived, and could
    /// not change anything else about it at all.
    fn provider(&self) -> Result<Box<dyn AgentProvider>, String> {
        let transport = HostTransport {
            context: self.context,
            send: self.send,
            free: self.free,
        };

        let choice = self
            .choice
            .lock()
            .map_err(|_| "the provider choice could not be read".to_owned())?
            .clone();
        let credential = self
            .credential
            .lock()
            .map_err(|_| "the credential could not be read".to_owned())?
            .clone();

        choice.build(credential.as_deref(), Box::new(transport))
    }
}

// SAFETY-adjacent, as for `HostTransport`: the only raw pointer here is the
// host's opaque context, which is never dereferenced on this side.
#[allow(unsafe_code)]
// SAFETY: `context` is opaque and never dereferenced; everything else is Send.
unsafe impl Send for Ephemeral {}
#[allow(unsafe_code)]
// SAFETY: as above, and every field a call touches is behind a `Mutex`.
unsafe impl Sync for Ephemeral {}

/// Opens Ephemeral at `home`, sending through the host's transport.
///
/// Returns null on failure. The handle must be released with
/// [`ephemeral_close`].
///
/// # Safety
///
/// `home` must be a NUL-terminated UTF-8 string. `send` and `free` must remain
/// callable, and `context` valid, for as long as the returned handle lives.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_open(
    home: *const c_char,
    send: EphemeralHttpSend,
    free: EphemeralHttpFree,
    context: *mut c_void,
) -> *mut Ephemeral {
    let Some(home) = (unsafe { string_from(home) }) else {
        return std::ptr::null_mut();
    };

    Box::into_raw(Box::new(Ephemeral {
        home: PathBuf::from(home),
        context,
        send,
        free,
        choice: Mutex::new(model::Choice::default()),
        credential: Mutex::new(None),
        last_error: Mutex::new(None),
    }))
}

/// Supplies the credential, from whatever secure store the platform provides.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]; `api_key` must be a
/// NUL-terminated UTF-8 string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_set_credential(
    handle: *mut Ephemeral,
    api_key: *const c_char,
) -> c_int {
    let Some(ephemeral) = (unsafe { handle.as_mut() }) else {
        return EPHEMERAL_BAD_HANDLE;
    };
    let Some(key) = (unsafe { string_from(api_key) }) else {
        ephemeral.remember("the credential was not readable text");
        return EPHEMERAL_ERROR;
    };

    let Ok(mut slot) = ephemeral.credential.lock() else {
        ephemeral.remember("the credential could not be stored");
        return EPHEMERAL_ERROR;
    };
    // An empty string means "there is none", so a host clearing a field does
    // not leave a credential that is present and blank.
    *slot = Some(key).filter(|key| !key.trim().is_empty());

    EPHEMERAL_OK
}

/// Chooses which service generates, and how it is configured.
///
/// `configuration_json` is `{"provider":"…"}` with optional `base_url`,
/// `model` and `ceiling`. [`ephemeral_providers`] lists what can be chosen and
/// what each one defaults to.
///
/// The credential is separate and stays where it was: it comes from the
/// platform's secure store through [`ephemeral_set_credential`], and this is
/// the part a host can keep in ordinary preferences.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]; `configuration_json` must be a
/// NUL-terminated UTF-8 string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_set_provider(
    handle: *mut Ephemeral,
    configuration_json: *const c_char,
) -> c_int {
    let Some(ephemeral) = (unsafe { handle.as_mut() }) else {
        return EPHEMERAL_BAD_HANDLE;
    };
    let Some(json) = (unsafe { string_from(configuration_json) }) else {
        ephemeral.remember("the provider configuration was not readable text");
        return EPHEMERAL_ERROR;
    };

    let chosen = match model::Choice::parse(&json) {
        Ok(chosen) => chosen,
        Err(reason) => {
            ephemeral.remember(&reason);
            return EPHEMERAL_ERROR;
        }
    };

    let Ok(mut slot) = ephemeral.choice.lock() else {
        ephemeral.remember("the provider choice could not be stored");
        return EPHEMERAL_ERROR;
    };
    *slot = chosen;

    EPHEMERAL_OK
}

/// What is currently chosen, as the same JSON [`ephemeral_set_provider`] takes.
///
/// Carries no credential, by construction rather than by redaction — the
/// credential is not part of the choice.
///
/// Returns null on failure. Free with [`ephemeral_string_free`].
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_provider(handle: *mut Ephemeral) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let choice = ephemeral
            .choice
            .lock()
            .map_err(|_| "the provider choice could not be read".to_owned())?;

        json(&*choice)
    })
}

/// Turns a filled-in form into the arguments the application receives.
///
/// `answers_json` is `{"input name": "what somebody typed", …}`. The result is
/// a JSON array of strings, or null with the reason in
/// [`ephemeral_last_error`] — in the words the person filling the form in needs
/// ("The earlier file is needed before this can run"), because those come from
/// the domain rather than from anything this crate invents.
///
/// A client could build this itself from the declaration on the page. It must
/// not: a phone, a window and a terminal composing argument vectors separately
/// are three subtly different applications, and the one that gets a flag's
/// default wrong sends a program the opposite of what somebody chose.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]; `id` and `answers_json` must be
/// NUL-terminated UTF-8 strings.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_arguments(
    handle: *mut Ephemeral,
    id: *const c_char,
    answers_json: *const c_char,
) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let (Some(id), Some(answers)) = (unsafe { string_from(id) }, unsafe {
        string_from(answers_json)
    }) else {
        ephemeral.remember("the form could not be read");
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let workspace = ephemeral.workspace()?;
        let app =
            AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;
        let manifest = workspace
            .apps()
            .load(&app)
            .map_err(|_| format!("there is no application called {id}"))?;

        let inputs = manifest
            .runtime
            .as_ref()
            .map(|runtime| runtime.inputs.clone())
            .unwrap_or_default();

        let answers: std::collections::BTreeMap<String, String> = serde_json::from_str(&answers)
            .map_err(|error| format!("that is not a form: {error}"))?;

        let built = ephemeral_core::manifest::arguments(&inputs, &answers)
            .map_err(|refusal| refusal.to_string())?;

        json(&built)
    })
}

/// Runs an application on this device, and says what it did.
///
/// `arguments_json` is the array [`ephemeral_arguments`] returned — composed by
/// the domain from a filled-in form, never assembled by a host. Returns
///
/// ```json
/// {"succeeded": true, "exit_code": 0, "output": "…", "refused": []}
/// ```
///
/// or null with the reason in [`ephemeral_last_error`]. Free the result with
/// [`ephemeral_string_free`].
///
/// **An application that runs and fails is not a failure of this call.** A
/// non-zero `exit_code` with the program's output is the answer; null means the
/// application never ran, and the reason says which of the several possible
/// causes it was — not generated, not WebAssembly, no interpreter installed, or
/// asking for something it was not granted.
///
/// ## This blocks
///
/// It runs the application to completion on the calling thread. A host must not
/// call it from a thread that draws — thirty seconds is the ceiling, and thirty
/// seconds of a frozen interface is worse than any answer is good.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]; `id` and `arguments_json` must
/// be NUL-terminated UTF-8 strings.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_run(
    handle: *mut Ephemeral,
    id: *const c_char,
    arguments_json: *const c_char,
) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let (Some(id), Some(arguments)) = (unsafe { string_from(id) }, unsafe {
        string_from(arguments_json)
    }) else {
        ephemeral.remember("the application and its arguments could not be read");
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let workspace = ephemeral.workspace()?;
        let app =
            AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;

        let arguments: Vec<String> = serde_json::from_str(&arguments)
            .map_err(|error| format!("those are not arguments: {error}"))?;

        json(&run::run(&workspace, &app, arguments)?)
    })
}

/// What the chosen service says it can be asked for.
///
/// The connection test, and the model list, in one call — because they have one
/// answer. It uses the endpoint and the credential generation would use, so a
/// wrong key, a base URL pointing at nothing or a retired model all show up
/// here rather than in the middle of a generation somebody is paying for.
///
/// Returns a JSON array of `{"id","name","ceiling"}`, or null on failure with
/// the service's own words in [`ephemeral_last_error`]. Free with
/// [`ephemeral_string_free`].
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_models(handle: *mut Ephemeral) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let listed = ephemeral
            .provider()?
            .models()
            .map_err(|error| error.to_string())?;

        json(&listed)
    })
}

/// Every provider this build can be pointed at, with what each one needs.
///
/// A host builds its picker from this rather than from a list of its own: an
/// application ships on its own schedule, and a hardcoded list of providers is
/// a list that is wrong the moment one is added.
///
/// Needs no handle — it is the same answer before anything is open.
///
/// Returns null on failure. Free with [`ephemeral_string_free`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn ephemeral_providers() -> *mut c_char {
    serde_json::to_string(&model::catalogue()).map_or(std::ptr::null_mut(), |text| owned(&text))
}

/// Closes Ephemeral and releases the handle.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`] and must not be used afterwards.
/// Passing null is allowed and does nothing.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_close(handle: *mut Ephemeral) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Why the last call failed, or null if it did not.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]. The result is owned by the
/// caller and must be released with [`ephemeral_string_free`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_last_error(handle: *mut Ephemeral) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };

    ephemeral
        .last_error
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .map_or(std::ptr::null_mut(), |reason| owned(&reason))
}

/// Releases a string this library returned.
///
/// # Safety
///
/// `text` must have come from this library and must not be used afterwards.
/// Passing null is allowed and does nothing.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}

/// Records a new application from what somebody asked for.
///
/// Returns its summary as JSON, or null on failure.
///
/// # Safety
///
/// `handle` must come from [`ephemeral_open`]; `intent` must be a
/// NUL-terminated UTF-8 string. The result must be released with
/// [`ephemeral_string_free`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_create(
    handle: *mut Ephemeral,
    intent: *const c_char,
) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let Some(intent) = (unsafe { string_from(intent) }) else {
        ephemeral.remember("what you asked for was not readable text");
        return std::ptr::null_mut();
    };

    guard(ephemeral, || create(ephemeral, &intent))
}

/// Every application, most recently touched first, as JSON.
///
/// # Safety
///
/// As [`ephemeral_create`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_applications(handle: *mut Ephemeral) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let workspace = ephemeral.workspace()?;
        let loaded = workspace
            .load_all()
            .map_err(|error| format!("could not read your applications: {error}"))?;

        json(&ephemeral_api::applications(
            &loaded.loaded,
            workspace.ledger(),
        ))
    })
}

/// One application's page, as JSON.
///
/// # Safety
///
/// As [`ephemeral_create`]; `id` must be a NUL-terminated UTF-8 string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_application(
    handle: *mut Ephemeral,
    id: *const c_char,
) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let Some(id) = (unsafe { string_from(id) }) else {
        ephemeral.remember("that is not an application id");
        return std::ptr::null_mut();
    };

    guard(ephemeral, || {
        let workspace = ephemeral.workspace()?;
        let app =
            AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;
        let manifest = workspace
            .apps()
            .load(&app)
            .map_err(|_| format!("there is no application called {id}"))?;

        json(&ephemeral_api::application(&manifest, &workspace))
    })
}

/// Plans and generates an application, writing its source.
///
/// Deliberately does **not** build, run or test it: a phone has no sandbox, and
/// running generated code outside one is the thing Ephemeral exists to prevent.
/// The application is left generated-and-unbuilt for a machine that can finish
/// it.
///
/// # Safety
///
/// As [`ephemeral_application`].
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_generate(
    handle: *mut Ephemeral,
    id: *const c_char,
) -> *mut c_char {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let Some(id) = (unsafe { string_from(id) }) else {
        ephemeral.remember("that is not an application id");
        return std::ptr::null_mut();
    };

    guard(ephemeral, || generate(ephemeral, &id))
}

/// Records a person's answer to one thing an application asked for.
///
/// # Safety
///
/// As [`ephemeral_application`]; `capability` must be a NUL-terminated UTF-8
/// string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ephemeral_decide(
    handle: *mut Ephemeral,
    id: *const c_char,
    capability: *const c_char,
    allow: bool,
) -> c_int {
    let Some(ephemeral) = (unsafe { handle.as_ref() }) else {
        return EPHEMERAL_BAD_HANDLE;
    };
    let (Some(id), Some(capability)) = (unsafe { string_from(id) }, unsafe {
        string_from(capability)
    }) else {
        ephemeral.remember("the decision was not readable text");
        return EPHEMERAL_ERROR;
    };

    match catch_unwind(AssertUnwindSafe(|| {
        decide(ephemeral, &id, &capability, allow)
    })) {
        Ok(Ok(())) => EPHEMERAL_OK,
        Ok(Err(reason)) => {
            ephemeral.remember(&reason);
            EPHEMERAL_ERROR
        }
        Err(_) => {
            ephemeral.remember("Ephemeral hit a bug and stopped rather than continuing");
            EPHEMERAL_ERROR
        }
    }
}

/// Runs an operation, turning failure and panic alike into a null return.
///
/// An unwind through a C frame is undefined behaviour, so it is caught here
/// rather than left to escape.
fn guard(
    ephemeral: &Ephemeral,
    operation: impl FnOnce() -> Result<*mut c_char, String>,
) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(text)) => text,
        Ok(Err(reason)) => {
            ephemeral.remember(&reason);
            std::ptr::null_mut()
        }
        Err(_) => {
            ephemeral.remember("Ephemeral hit a bug and stopped rather than continuing");
            std::ptr::null_mut()
        }
    }
}

fn create(ephemeral: &Ephemeral, intent: &str) -> Result<*mut c_char, String> {
    let mut workspace = ephemeral.workspace()?;

    // The whole operation, from `ephemeral-api`, rather than this crate's own
    // arrangement of the same steps. The arrangement it had before omitted the
    // audit entry, so an application created on a phone existed with no record
    // of anybody having asked for it.
    let manifest = ephemeral_api::create(
        &mut workspace,
        intent,
        None,
        ephemeral_core::retention::RetentionPolicy::default(),
    )?;

    json(&ephemeral_api::ApplicationSummary::of(
        &manifest,
        workspace.ledger(),
    ))
}

fn generate(ephemeral: &Ephemeral, id: &str) -> Result<*mut c_char, String> {
    let mut workspace = ephemeral.workspace()?;
    let app =
        AppId::parse(id).map_err(|error| format!("{id} is not an application id: {error}"))?;
    let mut manifest = workspace
        .apps()
        .load(&app)
        .map_err(|_| format!("there is no application called {id}"))?;

    // One provider for the whole of this generation. Built here rather than
    // held, so a host that changed its mind between calls gets what it chose —
    // and so that the plan and the code it turns into cannot come from two
    // different services.
    let provider = ephemeral.provider()?;

    provider.availability().map_err(|error| error.to_string())?;

    let plan = provider
        .plan(&manifest.description)
        .map_err(|error| error.to_string())?;

    manifest
        .apply(TransitionRequest::new(
            ephemeral_core::LifecycleEvent::Plan,
            Actor::Ephemeral,
            "asked for on a phone",
        ))
        .map_err(|error| format!("could not start planning: {error}"))?;
    manifest
        .apply(TransitionRequest::new(
            ephemeral_core::LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
            &plan.result.summary,
        ))
        .map_err(|error| format!("could not record the plan: {error}"))?;

    let generated = provider
        .generate(&plan.result)
        .map_err(|error| error.to_string())?;

    // Set before generation completes: an application that is building must
    // know what it runs on, and the manifest is validated on that transition.
    manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec {
        kind: plan.result.runtime,
        image: Some(plan.result.image.clone()),
        program: None,
        version: None,
        entrypoint: generated.result.entrypoint.clone(),
        interface: plan.result.interface,
        port: None,
        inputs: generated.result.inputs.clone(),
    });

    let source = workspace.layout().app(&manifest.id).source();
    std::fs::create_dir_all(&source)
        .map_err(|error| format!("could not create {}: {error}", source.display()))?;

    for file in &generated.result.files {
        let target = source.join(&file.path);
        // Refused rather than normalised. A generated path that climbs out of
        // the application is not a path to fix, it is a reason to stop.
        if !target.starts_with(&source) {
            return Err(format!("{} is outside the application", file.path));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        std::fs::write(&target, &file.contents)
            .map_err(|error| format!("could not write {}: {error}", target.display()))?;
    }

    manifest
        .apply(TransitionRequest::new(
            ephemeral_core::LifecycleEvent::GenerationCompleted,
            Actor::Agent,
            "written on a phone; not built here",
        ))
        .map_err(|error| format!("could not record generation: {error}"))?;

    // Every capability the plan asked for is recorded as a request, granted
    // nothing. A phone cannot quietly widen what an application may do.
    manifest.permissions = requested_permissions(&generated.result);
    manifest.rationale = generated
        .result
        .plan
        .requests
        .iter()
        .map(|request| ephemeral_core::manifest::PermissionRationale {
            permission: request.permission.clone(),
            reason: request.reason.clone(),
        })
        .collect();

    workspace
        .apps_mut()
        .save(&manifest)
        .map_err(|error| format!("could not save {id}: {error}"))?;
    workspace
        .save()
        .map_err(|error| format!("could not save: {error}"))?;

    json(&ephemeral_api::application(&manifest, &workspace))
}

fn decide(ephemeral: &Ephemeral, id: &str, capability: &str, allow: bool) -> Result<(), String> {
    let mut workspace = ephemeral.workspace()?;
    let app =
        AppId::parse(id).map_err(|error| format!("{id} is not an application id: {error}"))?;
    let manifest = workspace
        .apps()
        .load(&app)
        .map_err(|_| format!("there is no application called {id}"))?;

    // Matched against what the application actually asked for rather than
    // rebuilt from the string the host sent. A client that could compose a
    // permission out of a name would be a client that can grant something
    // nobody requested.
    let permission = manifest
        .permissions
        .capabilities()
        .into_iter()
        .find(|permission| permission.capability() == capability)
        .ok_or_else(|| format!("{id} has not asked for {capability}"))?;

    let subject = ephemeral_core::Principal::app(app.clone());
    let decided = ephemeral_core::permission::Permission::App(permission.clone());
    let reason = manifest
        .reason_for(&permission)
        .unwrap_or("no reason given")
        .to_owned();

    let ledger = workspace.ledger_mut();
    if allow {
        ledger.allow(subject.clone(), decided.clone(), Actor::User, reason)
    } else {
        ledger.deny(
            subject.clone(),
            decided.clone(),
            Actor::User,
            "declined on a phone",
        )
    }
    .map_err(|error| format!("that decision could not be recorded: {error}"))?;

    workspace.audit_mut().append(
        Actor::User,
        ephemeral_core::audit::AuditEvent::PermissionDecided {
            principal: subject,
            permission: decided,
            decision: if allow {
                ephemeral_core::permission::Decision::Allow
            } else {
                ephemeral_core::permission::Decision::Deny
            },
        },
    );

    workspace
        .save()
        .map_err(|error| format!("that decision could not be saved: {error}"))
}

/// Every capability a generated application asked for, granted nothing.
fn requested_permissions(
    app: &ephemeral_agent::GeneratedApp,
) -> ephemeral_core::permission::AppPermissions {
    let mut permissions = ephemeral_core::permission::AppPermissions::default();
    for request in &app.plan.requests {
        permissions.request(&request.permission);
    }
    permissions
}

/// Serialises a view for the host.
fn json<T: ::serde::Serialize>(value: &T) -> Result<*mut c_char, String> {
    let text =
        serde_json::to_string(value).map_err(|error| format!("could not serialise: {error}"))?;
    Ok(owned(&text))
}

/// Hands a string to the caller, who frees it with [`ephemeral_string_free`].
fn owned(text: &str) -> *mut c_char {
    CString::new(text).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Reads a C string the host passed in.
///
/// # Safety
///
/// `text` must be null or a NUL-terminated UTF-8 string valid for this call.
#[allow(unsafe_code)]
unsafe fn string_from(text: *const c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }

    // SAFETY: non-null, and NUL-terminated by the caller's contract.
    unsafe { CStr::from_ptr(text) }
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // What the fake host replies with, per call. A phone drives this library
    // through exactly these two function pointers, so a test that supplies
    // them exercises the real boundary rather than a Rust-shaped
    // approximation of it. Thread-local because the test harness runs tests in
    // parallel and a shared queue let them steal each other's replies.
    thread_local! {
        static REPLIES: std::cell::RefCell<Vec<String>> =
            const { std::cell::RefCell::new(Vec::new()) };

        /// Where the host was asked to send, and with what headers. Recorded
        /// rather than ignored, because "which service did this actually
        /// reach" is now a question with more than one possible answer, and it
        /// is the question the whole provider seam exists to let a person
        /// decide.
        static SENT: std::cell::RefCell<Vec<(String, String)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Queues what the fake host will reply, in the order it will be asked.
    fn queue(replies: Vec<String>) {
        REPLIES.with(|slot| *slot.borrow_mut() = replies.into_iter().rev().collect());
        SENT.with(|slot| slot.borrow_mut().clear());
    }

    /// The first request the host was asked to make: where, and with which
    /// headers.
    fn first_request() -> (String, Value) {
        let (endpoint, headers) =
            SENT.with(|slot| slot.borrow().first().cloned().expect("a request was sent"));
        (
            endpoint,
            serde_json::from_str(&headers).expect("JSON headers"),
        )
    }

    /// One header's value, matched case-insensitively as HTTP does.
    fn header(headers: &Value, wanted: &str) -> Option<String> {
        headers.as_array()?.iter().find_map(|header| {
            let name = header["name"].as_str()?;
            name.eq_ignore_ascii_case(wanted)
                .then(|| header["value"].as_str().unwrap_or_default().to_owned())
        })
    }

    extern "C" fn send(
        _context: *mut c_void,
        endpoint: *const c_char,
        headers: *const c_char,
        _request: *const c_char,
    ) -> *mut c_char {
        let read = |pointer: *const c_char| {
            if pointer.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        SENT.with(|slot| slot.borrow_mut().push((read(endpoint), read(headers))));

        let next = REPLIES.with(|slot| slot.borrow_mut().pop());
        next.map_or(std::ptr::null_mut(), |text| {
            CString::new(text).unwrap().into_raw()
        })
    }

    extern "C" fn free_reply(_context: *mut c_void, response: *mut c_char) {
        if !response.is_null() {
            drop(unsafe { CString::from_raw(response) });
        }
    }

    /// A model reply, framed the way Anthropic's API frames one.
    fn reply(payload: &str) -> String {
        serde_json::json!({
            "content": [{ "type": "text", "text": payload }],
            "usage": { "input_tokens": 1, "output_tokens": 2 },
        })
        .to_string()
    }

    /// The same, framed the way everything that copied OpenAI frames one.
    fn openai_reply(payload: &str) -> String {
        serde_json::json!({
            "choices": [{ "message": { "content": payload } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 2 },
        })
        .to_string()
    }

    fn open(home: &std::path::Path) -> *mut Ephemeral {
        let path = CString::new(home.to_str().unwrap()).unwrap();
        let handle =
            unsafe { ephemeral_open(path.as_ptr(), send, free_reply, std::ptr::null_mut()) };
        assert!(!handle.is_null());
        handle
    }

    fn text(pointer: *mut c_char) -> String {
        assert!(!pointer.is_null(), "expected a result, got null");
        let value = unsafe { CStr::from_ptr(pointer) }
            .to_str()
            .unwrap()
            .to_owned();
        unsafe { ephemeral_string_free(pointer) };
        value
    }

    fn c(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    /// Puts a runnable WebAssembly application on disk, generated-looking but
    /// without a model: a manifest declaring the wasm runtime, and a module in
    /// the application's own source.
    ///
    /// Written as WebAssembly text and assembled here rather than committed as
    /// a blob, because what the bytes do is the entire question.
    fn install(home: &std::path::Path, id: &str, text: &str) -> AppId {
        let app = AppId::parse(id).unwrap();
        let mut workspace = Workspace::open(home).unwrap();

        let mut manifest = ephemeral_core::AppManifest::requested(app.clone(), id);
        manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec::wasm_job(
            "program.wasm",
            Vec::new(),
        ));

        let source = workspace.layout().app(&app).source();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("program.wasm"),
            wat::parse_str(text).expect("the test application should assemble"),
        )
        .unwrap();

        std::fs::create_dir_all(workspace.layout().app(&app).root()).unwrap();
        workspace.apps_mut().save(&manifest).unwrap();

        app
    }

    /// An application that prints one fixed line, and nothing else.
    const SAYS_HELLO: &str = r#"(module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 32) "4 rows differ")
      (func (export "_start")
        (i32.store (i32.const 0) (i32.const 32))
        (i32.store (i32.const 4) (i32.const 13))
        (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 16)))))"#;

    /// **A phone runs an application.**
    ///
    /// The sentence this whole runtime exists to make true, asserted through
    /// the real C ABI a handset links against. Before this, `ephemeral_run` did
    /// not exist and could not have: running meant Docker, and no phone has
    /// Docker ([ADR-0021]).
    ///
    /// [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md
    #[test]
    fn a_phone_runs_an_application_and_gets_its_answer() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), "tally", SAYS_HELLO);
        let handle = open(home.path());

        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["succeeded"], true);
        assert_eq!(ran["exit_code"], 0);
        assert_eq!(ran["output"], "4 rows differ");

        unsafe { ephemeral_close(handle) };
    }

    /// An application that fails is an answer, not a failure of the call. A
    /// host that treated a non-zero exit as an error would hide every message
    /// a program writes about what went wrong.
    #[test]
    fn an_application_that_fails_still_answers() {
        let home = tempfile::tempdir().unwrap();
        install(
            home.path(),
            "tally",
            r#"(module
                 (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
                 (memory (export "memory") 1)
                 (func (export "_start") (call $exit (i32.const 3))))"#,
        );
        let handle = open(home.path());

        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["succeeded"], false);
        assert_eq!(ran["exit_code"], 3, "the program's own code, kept");

        unsafe { ephemeral_close(handle) };
    }

    /// An application that has not been generated has nothing to run, and is
    /// told so in words that say what to do rather than what failed.
    #[test]
    fn an_application_with_nothing_to_run_says_what_to_do_about_it() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        let created =
            text(unsafe { ephemeral_create(handle, c("compare two CSV files").as_ptr()) });
        let created: Value = serde_json::from_str(&created).unwrap();
        let id = c(created["id"].as_str().expect("an id"));

        let refused = unsafe { ephemeral_run(handle, id.as_ptr(), c("[]").as_ptr()) };
        assert!(refused.is_null(), "there is nothing to run");

        let said = text(unsafe { ephemeral_last_error(handle) });
        assert!(said.contains("generate"), "{said}");

        unsafe { ephemeral_close(handle) };
    }

    /// A Docker application on a phone is refused with the reason, not with a
    /// crash and not by pretending to run. Somebody holding a handset needs to
    /// know their application needs a computer, and why.
    #[test]
    fn a_container_application_is_refused_on_a_device_with_the_reason() {
        let home = tempfile::tempdir().unwrap();
        let app = AppId::parse("tally").unwrap();
        let mut workspace = Workspace::open(home.path()).unwrap();
        let mut manifest = ephemeral_core::AppManifest::requested(app.clone(), "tally");
        manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec::docker_job(
            "python:3.12-slim",
            vec!["python".to_owned(), "main.py".to_owned()],
        ));
        std::fs::create_dir_all(workspace.layout().app(&app).root()).unwrap();
        workspace.apps_mut().save(&manifest).unwrap();

        let handle = open(home.path());
        let refused = unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) };
        assert!(refused.is_null());

        let said = text(unsafe { ephemeral_last_error(handle) });
        assert!(said.contains("docker"), "{said}");
        assert!(said.contains("Docker"), "and what it would take: {said}");

        unsafe { ephemeral_close(handle) };
    }

    /// The form's answers reach the program as its arguments, composed by the
    /// domain rather than by the host — which is the only reason a phone, a
    /// window and a terminal run the same application the same way.
    #[test]
    fn what_somebody_typed_reaches_the_program() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), "tally", ECHOES_ITS_ARGUMENTS);
        let handle = open(home.path());

        let ran = text(unsafe {
            ephemeral_run(
                handle,
                c("tally").as_ptr(),
                c(r#"["--count","rows"]"#).as_ptr(),
            )
        });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["output"], "--count\nrows\n");

        unsafe { ephemeral_close(handle) };
    }

    /// Prints its own arguments, one per line, skipping argument zero.
    const ECHOES_ITS_ARGUMENTS: &str = r#"(module
      (import "wasi_snapshot_preview1" "args_sizes_get"
        (func $sizes (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "args_get"
        (func $args (param i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 2)
      (global $count (mut i32) (i32.const 0))
      (global $at (mut i32) (i32.const 0))
      (func $print_at (param $pointer i32)
        (local $end i32)
        (local.set $end (local.get $pointer))
        (block $found
          (loop $scan
            (br_if $found (i32.eqz (i32.load8_u (local.get $end))))
            (local.set $end (i32.add (local.get $end) (i32.const 1)))
            (br $scan)))
        (i32.store8 (local.get $end) (i32.const 10))
        (i32.store (i32.const 8) (local.get $pointer))
        (i32.store (i32.const 12)
          (i32.add (i32.sub (local.get $end) (local.get $pointer)) (i32.const 1)))
        (drop (call $write (i32.const 1) (i32.const 8) (i32.const 1) (i32.const 24))))
      (func (export "_start")
        (drop (call $sizes (i32.const 64) (i32.const 68)))
        (global.set $count (i32.load (i32.const 64)))
        (drop (call $args (i32.const 1024) (i32.const 2048)))
        (global.set $at (i32.const 1))
        (block $done
          (loop $next
            (br_if $done (i32.ge_u (global.get $at) (global.get $count)))
            (call $print_at
              (i32.load (i32.add (i32.const 1024)
                                 (i32.mul (global.get $at) (i32.const 4)))))
            (global.set $at (i32.add (global.get $at) (i32.const 1)))
            (br $next))))
    )"#;

    /// Creating an application needs no credential, no network, and no
    /// sandbox — which is why a phone can do it.
    #[test]
    fn an_application_can_be_created_on_a_device() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());

        let summary =
            text(unsafe { ephemeral_create(handle, c("compare two CSV files").as_ptr()) });
        let summary: Value = serde_json::from_str(&summary).unwrap();

        assert_eq!(summary["name"], "Compare Two CSV Files");
        assert_eq!(summary["state_kind"], "working");
        assert_eq!(summary["granted"], 0);

        let listed = text(unsafe { ephemeral_applications(handle) });
        let listed: Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        unsafe { ephemeral_close(handle) };
    }

    /// The default is still Anthropic, and it still sends Anthropic's headers.
    ///
    /// Paired with the test below rather than assumed: the point of the change
    /// is that a host *chooses*, and a choice only means something if the two
    /// choices produce visibly different requests.
    #[test]
    fn the_default_reaches_anthropic() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-ant-not-real").as_ptr()) };

        generate_something(handle, reply);

        let (endpoint, headers) = first_request();
        assert_eq!(endpoint, "https://api.anthropic.com/v1/messages");
        assert_eq!(
            header(&headers, "x-api-key").as_deref(),
            Some("sk-ant-not-real")
        );
        assert_eq!(
            header(&headers, "anthropic-version").as_deref(),
            Some("2023-06-01")
        );

        unsafe { ephemeral_close(handle) };
    }

    /// A phone, generating with Groq.
    ///
    /// This is the test the whole provider seam exists for. Before it, the C
    /// ABI took a bare `api_key` and the host wrapped it in Anthropic's
    /// headers, so no configuration anywhere could send a request to anybody
    /// else — the platform decided the vendor, which is not a decision a
    /// platform should be making.
    ///
    /// It asserts the two things that were impossible: the request goes to the
    /// URL that was chosen, and it carries the credential the way *that*
    /// service wants it rather than the way Anthropic does.
    #[test]
    fn a_phone_can_generate_with_groq() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());

        let chosen = c(r#"{"provider":"openai",
                           "base_url":"https://api.groq.com/openai/v1",
                           "model":"llama-3.3-70b-versatile"}"#);
        assert_eq!(
            unsafe { ephemeral_set_provider(handle, chosen.as_ptr()) },
            EPHEMERAL_OK,
            "{}",
            text(unsafe { ephemeral_last_error(handle) })
        );
        unsafe { ephemeral_set_credential(handle, c("gsk-not-real").as_ptr()) };

        generate_something(handle, openai_reply);

        let (endpoint, headers) = first_request();
        assert_eq!(endpoint, "https://api.groq.com/openai/v1/chat/completions");
        assert_eq!(
            header(&headers, "authorization").as_deref(),
            Some("Bearer gsk-not-real"),
            "an OpenAI-compatible service wants a bearer token, not an x-api-key"
        );
        assert!(
            header(&headers, "x-api-key").is_none(),
            "and it must not also be sent Anthropic's header"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// What is chosen can be read back, so a host can show it without keeping
    /// its own copy that drifts. The credential is not in it, by construction.
    #[test]
    fn what_was_chosen_can_be_read_back_without_the_credential() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("gsk-not-real").as_ptr()) };

        let chosen = c(r#"{"provider":"openai","model":"kimi-k2"}"#);
        unsafe { ephemeral_set_provider(handle, chosen.as_ptr()) };

        let read: Value =
            serde_json::from_str(&text(unsafe { ephemeral_provider(handle) })).unwrap();

        assert_eq!(read["provider"], "openai");
        assert_eq!(read["model"], "kimi-k2");
        assert!(
            !read.to_string().contains("gsk-not-real"),
            "the credential is not part of the choice and must not appear in it"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// A provider that does not exist changes nothing. A phone that quietly
    /// carried on with the previous service after somebody chose a different
    /// one would be generating with a company they did not pick.
    #[test]
    fn a_provider_that_does_not_exist_leaves_the_choice_alone() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());

        let nonsense = c(r#"{"provider":"gorq"}"#);
        assert_eq!(
            unsafe { ephemeral_set_provider(handle, nonsense.as_ptr()) },
            EPHEMERAL_ERROR
        );

        let why = text(unsafe { ephemeral_last_error(handle) });
        assert!(why.contains("gorq"), "{why}");

        let read: Value =
            serde_json::from_str(&text(unsafe { ephemeral_provider(handle) })).unwrap();
        assert_eq!(read["provider"], "anthropic", "the choice is unchanged");

        unsafe { ephemeral_close(handle) };
    }

    /// The catalogue a host builds its picker from is readable before anything
    /// is open, because a person may want to choose before they have a
    /// workspace.
    #[test]
    fn the_catalogue_is_readable_with_no_handle() {
        let listed: Value = serde_json::from_str(&text(ephemeral_providers())).unwrap();
        let names: Vec<&str> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|one| one["name"].as_str().unwrap())
            .collect();

        assert!(names.contains(&"openai"), "{names:?}");
        assert!(names.contains(&"anthropic"), "{names:?}");
        assert!(names.contains(&"mock"), "{names:?}");
    }

    /// Creates an application and generates it, with two replies framed by
    /// whichever provider is in use. The part every generation test needs and
    /// none of them is about.
    fn generate_something(handle: *mut Ephemeral, frame: fn(&str) -> String) {
        let created = text(unsafe { ephemeral_create(handle, c("count words").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        queue(vec![
            frame(
                r#"{"name":"Word Counter","summary":"counts words in a file",
                    "runtime":"docker","image":"python:3.12-slim",
                    "interface":"command_line","requests":[]}"#,
            ),
            frame(
                r#"{"files":[{"path":"main.py","contents":"print('hi')"}],
                    "dockerfile":"FROM python:3.12-slim\nCOPY . /app",
                    "entrypoint":["python","/app/main.py"],
                    "test_command":["python","-c","print(1)"]}"#,
            ),
        ]);

        let produced = unsafe { ephemeral_generate(handle, c(&id).as_ptr()) };
        assert!(
            !produced.is_null(),
            "generate failed: {}",
            text(unsafe { ephemeral_last_error(handle) })
        );
        unsafe { ephemeral_string_free(produced) };
    }

    /// The whole point of the exercise: plan and generate, on the device,
    /// through the host's own HTTPS.
    #[test]
    fn an_application_is_generated_through_the_hosts_transport() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-test-not-real").as_ptr()) };

        let created =
            text(unsafe { ephemeral_create(handle, c("count words in a file").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        queue(vec![
            reply(
                r#"{"name":"Word Counter","summary":"counts words in a file",
                    "runtime":"docker","image":"python:3.12-slim",
                    "interface":"command_line","requests":[]}"#,
            ),
            reply(
                r#"{"files":[{"path":"main.py","contents":"print('hi')"}],
                    "dockerfile":"FROM python:3.12-slim\nCOPY . /app",
                    "entrypoint":["python","/app/main.py"],
                    "test_command":["python","-c","print(1)"]}"#,
            ),
        ]);

        let produced = unsafe { ephemeral_generate(handle, c(&id).as_ptr()) };
        assert!(
            !produced.is_null(),
            "generate failed: {}",
            text(unsafe { ephemeral_last_error(handle) })
        );
        let detail: Value = serde_json::from_str(&text(produced)).unwrap();

        // Generated, and explicitly *not* built: a phone has no sandbox.
        assert_eq!(detail["summary"]["state_kind"], "working");
        assert!(
            home.path()
                .join("apps")
                .join(&id)
                .join("source/main.py")
                .exists(),
            "the source should have been written to the device"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// A phone must not be able to quietly widen what an application may do.
    #[test]
    fn generating_records_requests_and_grants_nothing() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-test-not-real").as_ptr()) };

        let created = text(unsafe { ephemeral_create(handle, c("read my downloads").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        queue(vec![
            reply(
                r#"{"name":"Reader","summary":"reads files","runtime":"docker",
                    "image":"python:3.12-slim","interface":"command_line",
                    "requests":[{"capability":"filesystem_read",
                                    "target":"~/Downloads/**",
                                    "reason":"to read the files you pick"}]}"#,
            ),
            reply(
                r#"{"files":[{"path":"main.py","contents":"x"}],
                    "dockerfile":"FROM python:3.12-slim",
                    "entrypoint":["python","/app/main.py"],
                    "test_command":["true"]}"#,
            ),
        ]);

        let detail = text(unsafe { ephemeral_generate(handle, c(&id).as_ptr()) });
        let detail: Value = serde_json::from_str(&detail).unwrap();

        assert_eq!(detail["summary"]["granted"], 0, "generation grants nothing");
        assert_eq!(
            detail["permissions"]["outstanding"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "it should be waiting on a person"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// A null handle is a status code, never a crash. A host that gets this
    /// wrong should get an error, not undefined behaviour.
    #[test]
    fn a_null_handle_is_refused_rather_than_dereferenced() {
        assert_eq!(
            unsafe {
                ephemeral_decide(
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    true,
                )
            },
            EPHEMERAL_BAD_HANDLE
        );
        assert!(unsafe { ephemeral_applications(std::ptr::null_mut()) }.is_null());
        assert!(unsafe { ephemeral_last_error(std::ptr::null_mut()) }.is_null());
        // Closing null is allowed and does nothing.
        unsafe { ephemeral_close(std::ptr::null_mut()) };
    }

    /// A failure has to be readable, or a host can only say "something broke".
    #[test]
    fn a_failure_is_reported_in_words() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());

        assert!(
            unsafe { ephemeral_application(handle, c("no-such-app-00000000").as_ptr()) }.is_null()
        );

        let reason = text(unsafe { ephemeral_last_error(handle) });
        assert!(reason.contains("no-such-app"), "{reason}");

        unsafe { ephemeral_close(handle) };
    }

    /// Generating without a credential is a diagnosis, not a mysterious
    /// network error — and nothing is sent.
    #[test]
    fn generation_without_a_credential_says_so() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());

        let created = text(unsafe { ephemeral_create(handle, c("do a thing").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        assert!(unsafe { ephemeral_generate(handle, c(&id).as_ptr()) }.is_null());
        let reason = text(unsafe { ephemeral_last_error(handle) });
        assert!(reason.contains("API key"), "{reason}");

        unsafe { ephemeral_close(handle) };
    }
}

#[cfg(test)]
mod header {
    /// The header a phone imports must name every symbol this crate exports.
    ///
    /// A hand-written header can drift from the library it describes, and the
    /// failure mode is a linker error on somebody else's machine, or worse a
    /// call through a signature that no longer matches. Cheap to check here.
    ///
    /// The list is read out of the source rather than written down twice. It
    /// used to be a literal array, which meant it checked only the symbols
    /// somebody remembered to add to it — a new export was undeclared and
    /// unnoticed, which is exactly the drift this test exists to catch.
    #[test]
    fn the_header_and_the_library_agree() {
        let header = include_str!("../include/ephemeral.h");
        let source = include_str!("lib.rs");

        let exported: Vec<&str> = source
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub "))
            .filter_map(|line| {
                line.strip_prefix("unsafe extern \"C\" fn ")
                    .or_else(|| line.strip_prefix("extern \"C\" fn "))
            })
            .filter_map(|rest| rest.split(['(', '<']).next())
            .collect();

        assert!(
            exported.len() >= 10,
            "the source was not read properly: {exported:?}"
        );

        // Declarations, not mentions. Asking whether the file *contains* the
        // name passes on a symbol that appears only in a comment about
        // another function — which is exactly what happened the first time
        // this was checked that way, and it reported agreement while the
        // declaration was missing.
        let declarations: Vec<&str> = header.lines().filter_map(declared).collect();

        for symbol in &exported {
            assert!(
                declarations.contains(symbol),
                "{symbol} is exported but the header does not declare it"
            );
        }

        // And the other way. A declaration with nothing behind it is a linker
        // error on the host's machine rather than on this one.
        for name in &declarations {
            assert!(
                exported.contains(name),
                "the header declares {name}, which the library does not export"
            );
        }

        for status in ["EPHEMERAL_OK", "EPHEMERAL_ERROR", "EPHEMERAL_BAD_HANDLE"] {
            assert!(
                header.contains(status),
                "{status} is missing from the header"
            );
        }
    }

    /// The name in a C declaration of one of ours, if the line is one.
    fn declared(line: &str) -> Option<&str> {
        let line = line.trim();
        if line.starts_with('*') || line.starts_with("/*") || line.starts_with("typedef") {
            return None;
        }

        let start = line.find("ephemeral_")?;
        let name = &line[start..];
        let name = &name[..name.find('(')?];

        // `ephemeral_open` is declared as `EphemeralHandle *ephemeral_open`, so
        // the pointer star is part of neither name.
        Some(name.trim_start_matches('*'))
    }
}
