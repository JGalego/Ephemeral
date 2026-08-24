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
    Actor, AppId, AppManifest,
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
/// `method` is `GET` or `POST`, and the host is to use exactly that. It crosses
/// the boundary because generation needs both: a `POST` asks a model for
/// something, a `GET` asks a service what models it has. A host that assumed
/// `POST` sent the listing request to `/v1/models` as a `POST` — which every
/// service refuses, most of them with an empty body, so the one call a person
/// makes before spending anything failed with a JSON parse error naming no
/// cause. `request_json` is empty for a `GET` and no body is to be sent.
///
/// `headers_json` is the complete header set the provider composed, in order,
/// as `[{"name":"…","value":"…"}, …]`. The host sets exactly these and adds
/// nothing: the credential is one of them, and so is whatever else the service
/// requires. It used to be a single `api_key` the host wrapped in Anthropic's
/// headers, which quietly made the ABI belong to one vendor — a phone could not
/// be pointed anywhere else no matter what anybody configured.
/// `status` is where the host writes the HTTP status it got — 200, 404,
/// whatever came back — or leaves as zero when it has none to report. It is
/// never null. A phone used to return a body and nothing else, so a confined
/// application reaching a service through this could not tell a refusal from an
/// answer; a caller that does not care about the status ignores it.
pub type EphemeralHttpSend = extern "C" fn(
    context: *mut c_void,
    method: *const c_char,
    endpoint: *const c_char,
    headers_json: *const c_char,
    request_json: *const c_char,
    status: *mut i32,
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

    /// Which service this transport is sending for.
    ///
    /// Carried so a failure is reported against the thing a person chose. Every
    /// failure here used to be attributed to `host`, which is not a provider,
    /// is not in the picker, and told somebody looking at "the API's reply was
    /// not JSON" nothing about which API.
    provider: String,
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
            provider: self.provider.clone(),
            reason,
        };

        let written = serde_json::to_string(&headers)
            .map_err(|error| failure(format!("the headers could not be written: {error}")))?;

        let method = CString::new(request.method.as_str())
            .map_err(|_| failure("the method is not a C string".to_owned()))?;
        let endpoint = CString::new(request.endpoint)
            .map_err(|_| failure("the endpoint is not a C string".to_owned()))?;
        let headers =
            CString::new(written).map_err(|_| failure("a header is not a C string".to_owned()))?;
        // Nothing at all for a GET, rather than the four characters `null` that
        // writing `Value::Null` produces. A host is told to send no body for a
        // GET, and handing it one it must remember to ignore is an invitation
        // to a bug on every platform separately.
        let sent = match request.method {
            ephemeral_agent::transport::Method::Get => String::new(),
            ephemeral_agent::transport::Method::Post => request.body.to_string(),
        };
        let body = CString::new(sent)
            .map_err(|_| failure("the request body is not a C string".to_owned()))?;

        // Read and deliberately unused: a provider's own reply says what went
        // wrong in its own words, which is more useful than a number, and every
        // one of them answers `{"error": …}` with a non-2xx status anyway.
        let mut status: i32 = 0;
        let reply = (self.send)(
            self.context,
            method.as_ptr(),
            endpoint.as_ptr(),
            headers.as_ptr(),
            body.as_ptr(),
            &raw mut status,
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

        // An empty reply is its own failure, not a parse error. A host that
        // gets a refusal with no body hands back an empty string, and reporting
        // that as "EOF while parsing a value at line 1 column 0" describes the
        // parser rather than the thing that went wrong.
        if copied.trim().is_empty() {
            return Err(failure(format!(
                "{} answered {} with nothing at all",
                request.endpoint,
                request.method.as_str()
            )));
        }

        serde_json::from_str(&copied).map_err(|error| ephemeral_agent::AgentError::Unreadable {
            provider: self.provider.clone(),
            reason: format!("the API's reply was not JSON: {error}"),
            raw: copied,
        })
    }
}

/// Carrying a confined application's request, on a handset.
///
/// The same seam, and the same host stack, that already carries a request to a
/// model provider — because a phone has exactly one HTTPS implementation worth
/// using and it is the platform's. An application never touches it: what
/// reaches here has already been checked against the grant by
/// [`ephemeral_runtime::wasm`], which is where the permission model lives and
/// where it stays.
///
/// The status the host reports travels with the body, so an application can
/// tell a refusal from an answer. A host with none to give leaves it zero,
/// which reaches the application as zero rather than as an invented `200` it
/// might branch on.
struct HostReach {
    context: *mut c_void,
    send: EphemeralHttpSend,
    free: EphemeralHttpFree,
}

// SAFETY-adjacent, as for `HostTransport`: `context` is opaque here and is only
// ever handed back to the host's own functions.
#[allow(unsafe_code)]
// SAFETY: the pointer is never dereferenced in this crate.
unsafe impl Send for HostReach {}
#[allow(unsafe_code)]
// SAFETY: as above.
unsafe impl Sync for HostReach {}

impl ephemeral_runtime::wasm::Reach for HostReach {
    fn fetch(
        &self,
        request: &ephemeral_runtime::wasm::Outbound,
    ) -> Result<ephemeral_runtime::wasm::Answered, String> {
        let method = CString::new(request.method.as_str())
            .map_err(|_| "the method is not a C string".to_owned())?;
        let endpoint = CString::new(request.url.as_str())
            .map_err(|_| "that address cannot be sent to this device's network".to_owned())?;
        // No headers at all. An application that could set one is an
        // application that can attach a credential it was never shown; the
        // content type is the host's business and everything else is nobody's.
        let headers = CString::new("[]").map_err(|_| "unreachable".to_owned())?;
        let body = CString::new(request.body.as_str())
            .map_err(|_| "that message cannot be sent as it is written".to_owned())?;

        let mut status: i32 = 0;
        let reply = (self.send)(
            self.context,
            method.as_ptr(),
            endpoint.as_ptr(),
            headers.as_ptr(),
            body.as_ptr(),
            &raw mut status,
        );

        if reply.is_null() {
            return Err(format!("{} could not be reached", request.url));
        }

        #[allow(unsafe_code)]
        // SAFETY: non-null, and by the documented contract of `EphemeralHttpSend`
        // a NUL-terminated string valid until `free` is called.
        let copied = unsafe { CStr::from_ptr(reply) }
            .to_string_lossy()
            .into_owned();
        (self.free)(self.context, reply);

        Ok(ephemeral_runtime::wasm::Answered {
            // Zero where the host had none to give, which is honest: an
            // invented 200 is a number an application might branch on.
            status: u16::try_from(status).unwrap_or(0),
            body: copied,
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
    /// What will carry a request an application was allowed to make.
    fn reach(&self) -> std::sync::Arc<dyn ephemeral_runtime::wasm::Reach> {
        std::sync::Arc::new(HostReach {
            context: self.context,
            send: self.send,
            free: self.free,
        })
    }

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
        let choice = self
            .choice
            .lock()
            .map_err(|_| "the provider choice could not be read".to_owned())?
            .clone();

        let transport = HostTransport {
            context: self.context,
            send: self.send,
            free: self.free,
            provider: choice.provider.clone(),
        };

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

        json(&run::run(&workspace, &app, arguments, ephemeral.reach())?)
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
    let container = plan.result.runtime == ephemeral_core::manifest::RuntimeKind::Docker;
    manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec {
        kind: plan.result.runtime,
        // An image is a container's business, and a script has none. Recording
        // one anyway would put a Docker tag on an application that will never
        // see Docker.
        image: container.then(|| plan.result.image.clone()),
        // Which file the interpreter runs. This is the field that turns what a
        // model just wrote into something this device can start: without it
        // `Program::locate` has nothing to look for, and the application comes
        // to rest saying it needs a computer.
        program: generated.result.program.clone(),
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
            "written on a phone",
        ))
        .map_err(|error| format!("could not record generation: {error}"))?;

    settle(&workspace, &mut manifest)?;

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

/// Puts a freshly generated application into the state it is actually in.
///
/// Generation ends at `Building`, and on a phone nothing builds. So every
/// application ever generated on a handset sat in `Building` for ever, under a
/// screen reading *"Ephemeral is building the app and setting up what it needs
/// to run"* — which was not true a millisecond after it was written, on a
/// device with nothing on it that could build anything.
///
/// There are two honest endings, and which one applies is a question about the
/// application rather than about the device it was written on:
///
/// * **WebAssembly that this device can run.** Nothing is left to do: a module
///   is already compiled, so `Building` and `Validating` are passed through
///   with reasons that say nothing was built or tested rather than borrowing
///   the container path's words. This is the same route
///   `ephemeral_engine::generation` takes for a WebAssembly recipe, for the
///   same reason.
///
/// * **Anything else.** A container image needs a container runtime and a
///   handset has none ([ADR-0021]). That is a blocker a person has to resolve
///   on another machine, and `Blocked` is the state that says exactly that —
///   *"Ephemeral cannot continue until something is resolved"* — with the
///   reason recorded beside it. The recipe is complete and portable
///   ([ADR-0012]); what is missing is somewhere to build it.
///
/// Neither ending is a failure. `BuildFailed` would claim a build was attempted
/// and did not work, and nothing was attempted.
///
/// Today every application generated on a phone takes the second route, because
/// generation still writes container applications whatever the device asking
/// for one can run. Teaching it to write for this runtime is its own piece of
/// work; this function is what that work arrives at, and it is asked the same
/// question either way.
///
/// [ADR-0012]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0012-sharing-distributes-recipes.md
/// [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md
fn settle(workspace: &Workspace, manifest: &mut AppManifest) -> Result<(), String> {
    // Whichever ending applies, decided before anything is recorded: the
    // question is about the application, and asking it half-way through a
    // sequence of transitions would be asking it about a different one.
    let next: Vec<(ephemeral_core::LifecycleEvent, Actor, String)> =
        match runnable_here(workspace, manifest) {
            Ok(checked) => vec![
                (
                    ephemeral_core::LifecycleEvent::BuildSucceeded,
                    Actor::Runtime,
                    "nothing to build: this runtime needs no build step".to_owned(),
                ),
                (
                    ephemeral_core::LifecycleEvent::ValidationPassed,
                    Actor::Runtime,
                    checked.to_owned(),
                ),
            ],
            Err(why) => vec![(ephemeral_core::LifecycleEvent::Block, Actor::Ephemeral, why)],
        };

    for (event, actor, why) in next {
        // Anything already applied is skipped rather than forced: an
        // application part-way along this route is being finished, not redone.
        if manifest.lifecycle.can_apply(event, actor) {
            manifest
                .apply(TransitionRequest::new(event, actor, &why))
                .map_err(|error| format!("could not record what happens next: {error}"))?;
        }
    }

    Ok(())
}

/// Whether this device can finish the application, or why it cannot.
///
/// The error is the sentence a person reads, so it says what is in the way and
/// where the rest of it can happen — not "unsupported runtime".
fn runnable_here(workspace: &Workspace, manifest: &AppManifest) -> Result<&'static str, String> {
    let Some(runtime) = &manifest.runtime else {
        return Err("it did not say what it runs on, so nothing here can finish it".to_owned());
    };

    if runtime.kind != ephemeral_core::manifest::RuntimeKind::Wasm {
        return Err(format!(
            "this device has no {} runtime, so it cannot build the app.              Everything needed to build it is written down and travels with it —              open it on a computer that has one.",
            runtime.kind.as_str()
        ));
    }

    let layout = workspace.layout();
    let program = ephemeral_runtime::wasm::Program::locate(
        runtime.program.as_deref(),
        &layout.app(&manifest.id).source(),
        &layout.interpreters_dir(),
    )
    .map_err(|error| error.to_string())?;

    let bytes = std::fs::read(program.wasm())
        .map_err(|error| format!("{} could not be read: {error}", program.wasm().display()))?;

    ephemeral_runtime::wasm::inspect(&bytes)
        .map_err(|error| format!("it is not something this device can run: {error}"))?;

    // Said differently for the two cases, because they are different claims.
    // A module that loads is the application itself checked. An interpreter
    // that loads is *the interpreter* checked, and all that has been
    // established about the script is that it is where the manifest says —
    // which is worth recording as exactly that rather than as "it works".
    Ok(match program {
        ephemeral_runtime::wasm::Program::Module { .. } => {
            "the module loads and has an entry point"
        }
        ephemeral_runtime::wasm::Program::Interpreted { .. } => {
            "its interpreter loads and its program is where it says it is; \
             nothing here ran it"
        }
    })
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
        static SENT: std::cell::RefCell<Vec<Asked>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// One request the fake host was asked to make.
    #[derive(Clone)]
    struct Asked {
        method: String,
        endpoint: String,
        headers: String,
        body: String,
    }

    /// Queues what the fake host will reply, in the order it will be asked.
    fn queue(replies: Vec<String>) {
        REPLIES.with(|slot| *slot.borrow_mut() = replies.into_iter().rev().collect());
        SENT.with(|slot| slot.borrow_mut().clear());
    }

    /// The first request the host was asked to make: where, and with which
    /// headers.
    fn first_request() -> (String, Value) {
        let asked = SENT.with(|slot| slot.borrow().first().cloned().expect("a request was sent"));
        (
            asked.endpoint,
            serde_json::from_str(&asked.headers).expect("JSON headers"),
        )
    }

    /// Every request the host was asked to make, in order.
    fn requests() -> Vec<Asked> {
        SENT.with(|slot| slot.borrow().clone())
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
        method: *const c_char,
        endpoint: *const c_char,
        headers: *const c_char,
        request: *const c_char,
        status: *mut i32,
    ) -> *mut c_char {
        assert!(!status.is_null(), "the ABI says this is never null");
        assert_eq!(
            unsafe { *status },
            0,
            "and that it arrives at zero, so a host with nothing to say says nothing"
        );
        // A teapot, so a test asserting the status cannot pass by accident on
        // a number something else would have produced.
        unsafe { *status = 418 };

        let read = |pointer: *const c_char| {
            if pointer.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        SENT.with(|slot| {
            slot.borrow_mut().push(Asked {
                method: read(method),
                endpoint: read(endpoint),
                headers: read(headers),
                body: read(request),
            });
        });

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
        install_as(home, id, text, ephemeral_core::manifest::AppInterface::Job)
    }

    fn install_as(
        home: &std::path::Path,
        id: &str,
        text: &str,
        interface: ephemeral_core::manifest::AppInterface,
    ) -> AppId {
        let app = AppId::parse(id).unwrap();
        let mut workspace = Workspace::open(home).unwrap();

        let mut manifest = ephemeral_core::AppManifest::requested(app.clone(), id);
        let mut runtime =
            ephemeral_core::manifest::RuntimeSpec::wasm_job("program.wasm", Vec::new());
        runtime.interface = interface;
        manifest.runtime = Some(runtime);

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

    /// Puts something named like an interpreter where the engine looks.
    ///
    /// Deliberately not the real one. What these tests are about is the
    /// lifecycle reaching `Ready` and the application being runnable here —
    /// that the JavaScript in it does what it says is Boa's business, and
    /// building three megabytes of engine to assert a state transition would
    /// make this suite depend on a cross-compiled artifact.
    fn install_interpreter(home: &std::path::Path) {
        let into = home.join("interpreters");
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(
            into.join("javascript.wasm"),
            wat::parse_str(r#"(module (memory (export "memory") 1) (func (export "_start")))"#)
                .expect("the stand-in should assemble"),
        )
        .unwrap();
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

    /// **A rejected key is a refusal, not a short list.**
    ///
    /// The connection test exists to tell "configured" from "working", and it
    /// used to paint a rejection green: a service that answers `{"error": …}`
    /// carries no `data`, parsing that yields zero models, and zero models was
    /// reported as success. A phone in a rack photographed it saying "Reached
    /// it. 0 models." a minute before generation failed for the same reason.
    #[test]
    fn a_service_that_rejects_the_key_is_not_reported_as_reached() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-not-a-real-key").as_ptr()) };

        // What a service actually answers when the key is wrong.
        queue(vec![
            serde_json::json!({
                "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" }
            })
            .to_string(),
        ]);

        let listed = unsafe { ephemeral_models(handle) };
        assert!(
            listed.is_null(),
            "a rejection must not come back as a listing"
        );

        let said = text(unsafe { ephemeral_last_error(handle) });
        assert!(
            said.contains("Incorrect API key"),
            "and it carries the service's own words: {said}"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// The same, on the provider that actually failed on the phone.
    ///
    /// The test above drives Anthropic, because that is what a handle defaults
    /// to — so on its own it would have left the OpenAI path, the one a rack
    /// phone broke on, with no boundary test at all. Both providers had the
    /// bug; both need the assertion.
    #[test]
    fn a_rejection_from_an_openai_service_is_a_refusal_too() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe {
            ephemeral_set_provider(handle, c(r#"{"provider":"openai"}"#).as_ptr());
            ephemeral_set_credential(handle, c("sk-not-a-real-key").as_ptr());
        }

        queue(vec![
            serde_json::json!({
                "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" }
            })
            .to_string(),
        ]);

        assert!(unsafe { ephemeral_models(handle) }.is_null());
        assert!(
            text(unsafe { ephemeral_last_error(handle) }).contains("Incorrect API key"),
            "the service's own words, whichever provider produced them"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// A service that answers with a listing is reached, however short the
    /// listing is. Zero models is a fact about an account, not an error — the
    /// distinction the fix above turns on.
    #[test]
    fn an_empty_listing_is_still_a_listing() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-not-a-real-key").as_ptr()) };

        queue(vec![serde_json::json!({ "data": [] }).to_string()]);

        let listed = text(unsafe { ephemeral_models(handle) });
        assert_eq!(listed, "[]", "reached, and it has nothing");

        unsafe { ephemeral_close(handle) };
    }

    /// **The listing is asked for as a `GET`, and generation as a `POST`.**
    ///
    /// The method crosses the boundary because both hosts assumed `POST` and
    /// neither could have been told otherwise. That sent the connection test to
    /// `/v1/models` as a `POST`, which OpenAI refuses with an empty body — so a
    /// real phone reported "the API's reply was not JSON" for a key and an
    /// endpoint that both worked, and generation against the same service
    /// succeeded seconds later.
    ///
    /// A `GET` carries no body at all rather than the four characters `null`,
    /// which is what writing out a JSON null produced.
    #[test]
    fn a_listing_is_a_get_with_no_body_and_generation_is_a_post() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-not-a-real-key").as_ptr()) };

        queue(vec![serde_json::json!({ "data": [] }).to_string()]);
        let _ = text(unsafe { ephemeral_models(handle) });

        let listing = requests();
        let listing = listing.first().expect("a listing was sent");
        assert_eq!(listing.method, "GET", "{}", listing.endpoint);
        assert_eq!(listing.body, "", "a GET carries no body, not the word null");

        let created =
            text(unsafe { ephemeral_create(handle, c("compare two CSV files").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .expect("an id")
            .to_owned();

        queue(vec![
            reply(
                r#"{"name":"CSV Comparator","summary":"compares two CSV files",
                    "interface":"command_line","requests":[]}"#,
            ),
            reply(
                r#"{"files":[{"path":"main.js","contents":"console.log('hi')"}],
                    "program":"main.js"}"#,
            ),
        ]);
        let generated = unsafe { ephemeral_generate(handle, c(&id).as_ptr()) };
        assert!(
            !generated.is_null(),
            "generate failed: {}",
            text(unsafe { ephemeral_last_error(handle) })
        );

        for asked in requests() {
            assert_eq!(
                asked.method, "POST",
                "asking a model for something is a POST: {}",
                asked.endpoint
            );
            assert!(
                asked.body.contains("\"model\""),
                "a POST carries the request body: {}",
                asked.body
            );
        }

        unsafe { ephemeral_close(handle) };
    }

    /// A reply with nothing in it is reported as nothing, against the service
    /// a person actually chose.
    ///
    /// Both halves are what a real phone got wrong at once: an empty body was
    /// described as a JSON parse error ("EOF while parsing a value at line 1
    /// column 0"), and it was attributed to `host`, which is not a provider,
    /// is not in the picker and is not a thing anybody selected.
    #[test]
    fn an_empty_reply_says_so_and_names_the_service() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe {
            ephemeral_set_provider(handle, c(r#"{"provider":"openai"}"#).as_ptr());
            ephemeral_set_credential(handle, c("sk-not-a-real-key").as_ptr());
        }

        queue(vec![String::new()]);
        assert!(unsafe { ephemeral_models(handle) }.is_null());

        let why = text(unsafe { ephemeral_last_error(handle) });
        assert!(
            why.contains("nothing at all"),
            "an empty reply is not a parse failure: {why}"
        );
        assert!(
            why.contains("openai"),
            "reported against the service that was chosen: {why}"
        );
        assert!(
            !why.contains("EOF while parsing"),
            "the parser is not what went wrong: {why}"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// **What a phone writes comes to rest somewhere true.**
    ///
    /// Generation ends at `Building`, and nothing on a handset builds. Every
    /// application ever generated on one therefore sat in `Building` for ever,
    /// under a screen reading *"Ephemeral is building the app and setting up
    /// what it needs to run"* — on a device with nothing on it that could.
    ///
    /// This asserts the question `settle` asks, on both answers. The container
    /// answer is what `ephemeral_generate` produces today and is covered end to
    /// end by
    /// [`an_application_is_generated_through_the_hosts_transport`]; the
    /// WebAssembly answer is asked here directly, because generation does not
    /// write for that runtime yet — the plans it produces are container plans,
    /// and teaching it otherwise is its own piece of work.
    #[test]
    fn what_a_device_can_finish_is_a_question_about_the_application() {
        let home = tempfile::tempdir().unwrap();
        let app = install(home.path(), "tally", SAYS_HELLO);
        let workspace = Workspace::open(home.path()).unwrap();

        let module = workspace.apps().load(&app).unwrap();
        assert!(
            runnable_here(&workspace, &module).is_ok(),
            "a compiled module needs nothing built and nothing installed"
        );

        // The same application, said to be a container. Nothing about the files
        // changed; what changed is what it claims to need.
        let mut container = module.clone();
        container.runtime = Some(ephemeral_core::manifest::RuntimeSpec {
            kind: ephemeral_core::manifest::RuntimeKind::Docker,
            image: Some("python:3.12-slim".to_owned()),
            ..module.runtime.clone().unwrap()
        });
        let why = runnable_here(&workspace, &container).expect_err("a phone has no Docker");
        assert!(why.contains("docker"), "{why}");
        assert!(
            why.contains("computer"),
            "it has to say where the rest of it can happen: {why}"
        );

        // And a WebAssembly application whose module is not there. Not a
        // failure of the device — a failure to say what to run.
        let mut nothing = module.clone();
        nothing.runtime.as_mut().unwrap().program = None;
        assert!(runnable_here(&workspace, &nothing).is_err());
    }

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

    /// A module that asks Ephemeral for one request and prints the answer.
    const ASKS_THE_NETWORK: &str = r#"(module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (import "ephemeral" "send" (func $send (param i32 i32) (result i32)))
      (import "ephemeral" "recv" (func $recv (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 1024) "{\"method\":\"GET\",\"url\":\"https://room.example.com/garden\"}")
      (func (export "_start")
        (local $n i32)
        (drop (call $send (i32.const 1024) (i32.const 56)))
        (local.set $n (call $recv (i32.const 2048) (i32.const 8192)))
        (i32.store (i32.const 0) (i32.const 2048))
        (i32.store (i32.const 4) (local.get $n))
        (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))))"#;

    /// Allows `app` to reach `host`, and Ephemeral itself to carry it.
    fn allow_network(home: &std::path::Path, app: &AppId, host: &str) {
        use ephemeral_core::Principal;
        use ephemeral_core::permission::{AppPermission, HostScope, MetaPermission};

        let mut workspace = Workspace::open(home).unwrap();
        let ledger = workspace.ledger_mut();
        ledger
            .allow(
                Principal::app(app.clone()),
                ephemeral_core::permission::Permission::App(AppPermission::NetworkOutbound {
                    scope: HostScope::parse(host).unwrap(),
                }),
                Actor::User,
                "the room they agreed on",
            )
            .unwrap();
        ledger
            .allow(
                Principal::Ephemeral,
                ephemeral_core::permission::Permission::Meta(MetaPermission::NetworkAccess),
                Actor::User,
                "so it can carry a request",
            )
            .unwrap();
        workspace.save().unwrap();
    }

    /// **A phone carries a confined application's request, and nothing else.**
    ///
    /// The sentence this whole seam exists to make true, asserted through the
    /// real C ABI a handset links against. A WebAssembly application has no
    /// socket and never will; what it has, once somebody allowed a destination,
    /// is the host's own HTTPS — the same callback that already reaches a model
    /// provider. Two phones can now hold a conversation without sharing a
    /// filesystem, which before this they could not.
    #[test]
    fn a_phone_carries_a_request_an_application_was_allowed_to_make() {
        let home = tempfile::tempdir().unwrap();
        let app = install(home.path(), "whisper", ASKS_THE_NETWORK);
        allow_network(home.path(), &app, "room.example.com");

        let handle = open(home.path());
        queue(vec!["Alice\tis this thing on?\n".to_owned()]);

        let ran = text(unsafe { ephemeral_run(handle, c("whisper").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["succeeded"], true, "{ran}");
        let output = ran["output"].as_str().unwrap_or_default();
        assert!(
            output.contains("is this thing on?"),
            "the other person's message reaches the application: {output}"
        );

        // Through the host's own transport, as a GET, with no headers — an
        // application that could set one could attach a credential it was
        // never shown.
        let asked = requests();
        let asked = asked.first().expect("the host was asked to make it");
        assert_eq!(asked.method, "GET");
        assert_eq!(asked.endpoint, "https://room.example.com/garden");
        assert_eq!(asked.headers, "[]", "an application composes no headers");

        // And the status the host reported reaches the application, so it can
        // tell a refusal from an answer. A phone used to hand back a body and
        // nothing else, which made every reply look like a success.
        assert!(
            output.contains("418"),
            "the status crosses the boundary with the body: {output}"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// **And nowhere else.**
    ///
    /// The same application, the same grant, a different destination. The
    /// refusal happens in the runtime, against the ledger, *before* the host is
    /// asked — so a phone's transport, which would happily fetch anything, is
    /// never given the chance.
    #[test]
    fn a_phone_is_never_asked_to_reach_what_nobody_allowed() {
        let home = tempfile::tempdir().unwrap();
        let app = install(home.path(), "whisper", ASKS_THE_NETWORK);
        allow_network(home.path(), &app, "somewhere.example.org");

        let handle = open(home.path());
        queue(vec!["should never be sent".to_owned()]);

        let ran = text(unsafe { ephemeral_run(handle, c("whisper").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        let output = ran["output"].as_str().unwrap_or_default();
        assert!(
            output.contains("not allowed to reach"),
            "it should have been refused: {output}"
        );
        assert!(
            requests().is_empty(),
            "and the host should never have been asked"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// **Tier one: an application that returns a page.**
    ///
    /// A WebAssembly application has no socket and cannot be a server, so a
    /// "web application" here is one that *writes* a page and lets the host
    /// render it. That is not a lesser version of the idea — it is why showing
    /// somebody a user interface costs no network permission at all.
    #[test]
    fn an_application_can_return_a_page_without_a_server() {
        let home = tempfile::tempdir().unwrap();
        install_as(
            home.path(),
            "tally",
            WRITES_A_PAGE,
            ephemeral_core::manifest::AppInterface::Web,
        );
        let handle = open(home.path());

        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["succeeded"], true);
        assert_eq!(
            ran["presentation"], "page",
            "so a host knows to render it rather than print it"
        );
        assert!(
            ran["output"].as_str().unwrap().contains("<h1>"),
            "{}",
            ran["output"]
        );

        unsafe { ephemeral_close(handle) };
    }

    /// The same output from an application that did not declare itself a web
    /// application is text. Deciding from the declaration rather than from the
    /// bytes is what stops a comparison whose first line happens to be markup
    /// from being rendered as a document.
    #[test]
    fn markup_from_an_application_that_is_not_a_page_stays_text() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), "tally", WRITES_A_PAGE);
        let handle = open(home.path());

        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert_eq!(ran["presentation"], "text");

        unsafe { ephemeral_close(handle) };
    }

    /// Writes a small page and stops.
    const WRITES_A_PAGE: &str = r#"(module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 32) "<h1>4 rows differ</h1>")
      (func (export "_start")
        (i32.store (i32.const 0) (i32.const 32))
        (i32.store (i32.const 4) (i32.const 22))
        (drop (call $write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 16)))))"#;

    /// **A phone is told when a permission it granted does nothing.**
    ///
    /// Both halves of the model reach a device, and nothing here mirrors the
    /// operating system's own permissions into the ledger yet — so an
    /// application can hold a grant Ephemeral has no authority to act on. Until
    /// this, a handset was the only client where an application found nothing
    /// and nothing said why; the terminal and the window have always shown it
    /// before a run.
    #[test]
    fn a_permission_that_does_nothing_says_so_when_the_application_runs() {
        let home = tempfile::tempdir().unwrap();
        let app = install(home.path(), "tally", SAYS_HELLO);

        // Asked for, then allowed — but only the application's half. Ephemeral
        // itself has not been allowed to read anything on somebody's behalf, so
        // the grant is real and does nothing, which is the case this is about.
        let wanted = ephemeral_core::permission::AppPermission::FilesystemRead {
            scope: ephemeral_core::permission::PathScope::parse("~/Documents").unwrap(),
        };
        let capability = wanted.capability().to_owned();
        let mut workspace = Workspace::open(home.path()).unwrap();
        let mut manifest = workspace.apps().load(&app).unwrap();
        manifest.permissions.request(&wanted);
        workspace.apps_mut().save(&manifest).unwrap();
        drop(workspace);

        let handle = open(home.path());
        assert_eq!(
            unsafe { ephemeral_decide(handle, c("tally").as_ptr(), c(&capability).as_ptr(), true) },
            EPHEMERAL_OK,
            "{}",
            text(unsafe { ephemeral_last_error(handle) })
        );
        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        let said = ran["inert"].as_str().unwrap_or_default();
        assert!(
            said.contains("do nothing"),
            "the phone was not told its grant is inert: {ran}"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// And nothing is said when there is nothing to say. A warning that fires
    /// when everything is fine is a warning nobody reads.
    #[test]
    fn an_application_holding_nothing_is_not_warned_about_nothing() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), "tally", SAYS_HELLO);
        let handle = open(home.path());

        let ran = text(unsafe { ephemeral_run(handle, c("tally").as_ptr(), c("[]").as_ptr()) });
        let ran: Value = serde_json::from_str(&ran).unwrap();

        assert!(ran["inert"].is_null(), "{ran}");

        unsafe { ephemeral_close(handle) };
    }

    /// The page carries which runtime an application declares.
    ///
    /// A phone reads this to say what it can do with the application in front
    /// of it. It was being sent all along and nothing read it, which is how
    /// both screens came to tell everybody a phone cannot run anything long
    /// after one could — so it is asserted now that something depends on it.
    #[test]
    fn an_applications_page_says_which_runtime_it_declares() {
        let home = tempfile::tempdir().unwrap();
        install(home.path(), "tally", SAYS_HELLO);
        let handle = open(home.path());

        let page = text(unsafe { ephemeral_application(handle, c("tally").as_ptr()) });
        let page: Value = serde_json::from_str(&page).unwrap();

        assert_eq!(page["runtime"]["kind"], "wasm");

        // And an application that has not been generated says nothing rather
        // than guessing, so a client can tell "runs here" from "not yet known".
        let created =
            text(unsafe { ephemeral_create(handle, c("compare two CSV files").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let page = text(unsafe { ephemeral_application(handle, c(&id).as_ptr()) });
        let page: Value = serde_json::from_str(&page).unwrap();
        assert!(page["runtime"].is_null(), "{}", page["runtime"]);

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
                    "interface":"command_line","requests":[]}"#,
            ),
            frame(
                r#"{"files":[{"path":"main.js","contents":"console.log('hi')"}],
                    "program":"main.js"}"#,
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
        install_interpreter(home.path());
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
                    "interface":"command_line","requests":[]}"#,
            ),
            reply(
                r#"{"files":[{"path":"main.js","contents":"console.log('hi')"}],
                    "program":"main.js"}"#,
            ),
        ]);

        let produced = unsafe { ephemeral_generate(handle, c(&id).as_ptr()) };
        assert!(
            !produced.is_null(),
            "generate failed: {}",
            text(unsafe { ephemeral_last_error(handle) })
        );
        let detail: Value = serde_json::from_str(&text(produced)).unwrap();

        // **Ready, on the device it was written on.** Not "Building" for ever,
        // not "Blocked" pointing at a computer somebody has to go and find:
        // the phone asked for a script because a script is what it can run,
        // and the interpreter to run it is already here.
        assert_eq!(detail["summary"]["state"], "Ready", "{detail}");
        assert_eq!(detail["summary"]["runnable"], true);
        assert_eq!(detail["runtime"]["kind"], "wasm");
        assert_eq!(detail["runtime"]["runs_locally"], true);
        assert_eq!(
            detail["runtime"]["image"],
            Value::Null,
            "a script has no base image, and recording one would be a Docker \
             tag on something that will never see Docker"
        );
        assert!(
            home.path()
                .join("apps")
                .join(&id)
                .join("source/main.js")
                .exists(),
            "the source should have been written to the device"
        );

        unsafe { ephemeral_close(handle) };
    }

    /// **Without the interpreter, it says which one and where to put it.**
    ///
    /// The other half of the sentence above. An application that is a script
    /// needs something on the device that runs scripts, and a device that has
    /// none should say so by name rather than fail at Run with a shrug.
    #[test]
    fn a_device_with_no_interpreter_names_the_one_it_wants() {
        let home = tempfile::tempdir().unwrap();
        let handle = open(home.path());
        unsafe { ephemeral_set_credential(handle, c("sk-test-not-real").as_ptr()) };

        let created = text(unsafe { ephemeral_create(handle, c("count words").as_ptr()) });
        let id = serde_json::from_str::<Value>(&created).unwrap()["id"]
            .as_str()
            .expect("an id")
            .to_owned();

        queue(vec![
            reply(r#"{"name":"Counter","summary":"counts","interface":"job","requests":[]}"#),
            reply(
                r#"{"files":[{"path":"main.js","contents":"console.log('hi')"}],
                    "program":"main.js"}"#,
            ),
        ]);

        let detail = text(unsafe { ephemeral_generate(handle, c(&id).as_ptr()) });
        let detail: Value = serde_json::from_str(&detail).unwrap();

        assert_eq!(detail["summary"]["state"], "Blocked");
        let explanation = detail["explanation"].as_str().unwrap_or_default();
        assert!(
            explanation.contains("JavaScript") && explanation.contains("javascript.wasm"),
            "it should name the interpreter and where it goes: {explanation}"
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
                r#"{"name":"Reader","summary":"reads files","interface":"command_line",
                    "requests":[{"capability":"filesystem_read",
                                    "target":"~/Downloads/**",
                                    "reason":"to read the files you pick"}]}"#,
            ),
            reply(
                r#"{"files":[{"path":"main.js","contents":"console.log('x')"}],
                    "program":"main.js"}"#,
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
