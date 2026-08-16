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
//! description, an HTTPS request, and some parsing. It cannot **build**, **run**
//! or **repair** one, because those need a sandbox no phone has ([ADR-0007]).
//!
//! An application generated here is therefore real and versioned, with its
//! source written and its requested permissions recorded, and its lifecycle
//! stops at "generated, not built" — a state the machine already models. A
//! machine that can build finishes the job.
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
use ephemeral_provider_anthropic::AnthropicProvider;
use serde_json::Value;

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
pub type EphemeralHttpSend = extern "C" fn(
    context: *mut c_void,
    endpoint: *const c_char,
    api_key: *const c_char,
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
        // The C callback takes a credential and lets the host add the rest of
        // the headers, which `ephemeral.h` documents. That is still true while
        // this builds an Anthropic provider and nothing else. Wiring a second
        // provider in here means passing the whole header set across the
        // boundary, which is a change to a published ABI and therefore a
        // decision rather than a detail.
        let endpoint = request.endpoint;
        let api_key = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
            .map_or("", |(_, value)| value.as_str());
        let request = request.body;

        let failure = |reason: String| ephemeral_agent::AgentError::Failed {
            provider: ephemeral_provider_anthropic::NAME.to_owned(),
            reason,
        };

        let endpoint = CString::new(endpoint)
            .map_err(|_| failure("the endpoint is not a C string".to_owned()))?;
        let key = CString::new(api_key)
            .map_err(|_| failure("the credential is not a C string".to_owned()))?;
        let body = CString::new(request.to_string())
            .map_err(|_| failure("the request body is not a C string".to_owned()))?;

        let reply = (self.send)(self.context, endpoint.as_ptr(), key.as_ptr(), body.as_ptr());

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
            provider: ephemeral_provider_anthropic::NAME.to_owned(),
            reason: format!("the API's reply was not JSON: {error}"),
            raw: copied,
        })
    }
}

/// One open Ephemeral, as the host sees it.
///
/// Opaque on purpose: the host holds a pointer and nothing else, so the layout
/// of everything inside can change without breaking a build somebody shipped.
pub struct Ephemeral {
    home: PathBuf,
    provider: AnthropicProvider,
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
}

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

    let transport = HostTransport {
        context,
        send,
        free,
    };

    Box::into_raw(Box::new(Ephemeral {
        home: PathBuf::from(home),
        provider: AnthropicProvider::with_transport(Box::new(transport)),
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

    // Replaced wholesale rather than mutated, because the provider owns its
    // transport and the credential is the only part a host changes.
    let replaced = std::mem::replace(
        &mut ephemeral.provider,
        AnthropicProvider::with_transport(Box::new(NoTransport)),
    );
    ephemeral.provider = replaced.with_credential(key);

    EPHEMERAL_OK
}

/// A placeholder used only while swapping a credential in.
struct NoTransport;

impl Transport for NoTransport {
    fn send(&self, _request: &HttpRequest<'_>) -> Result<Value, ephemeral_agent::AgentError> {
        Err(ephemeral_agent::AgentError::Failed {
            provider: ephemeral_provider_anthropic::NAME.to_owned(),
            reason: "no transport".to_owned(),
        })
    }
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

    ephemeral
        .provider
        .availability()
        .map_err(|error| error.to_string())?;

    let plan = ephemeral
        .provider
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

    let generated = ephemeral
        .provider
        .generate(&plan.result)
        .map_err(|error| error.to_string())?;

    // Set before generation completes: an application that is building must
    // know what it runs on, and the manifest is validated on that transition.
    manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec {
        kind: plan.result.runtime,
        image: Some(plan.result.image.clone()),
        version: None,
        entrypoint: generated.result.entrypoint.clone(),
        interface: plan.result.interface,
        port: None,
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
    }

    /// Queues what the fake host will reply, in the order it will be asked.
    fn queue(replies: Vec<String>) {
        REPLIES.with(|slot| *slot.borrow_mut() = replies.into_iter().rev().collect());
    }

    extern "C" fn send(
        _context: *mut c_void,
        _endpoint: *const c_char,
        _api_key: *const c_char,
        _request: *const c_char,
    ) -> *mut c_char {
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

    /// A model reply, framed the way the API frames one.
    fn reply(payload: &str) -> String {
        serde_json::json!({
            "content": [{ "type": "text", "text": payload }],
            "usage": { "input_tokens": 1, "output_tokens": 2 },
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
    #[test]
    fn the_header_and_the_library_agree() {
        let header = include_str!("../include/ephemeral.h");
        let source = include_str!("lib.rs");

        for symbol in [
            "ephemeral_open",
            "ephemeral_close",
            "ephemeral_set_credential",
            "ephemeral_last_error",
            "ephemeral_string_free",
            "ephemeral_create",
            "ephemeral_applications",
            "ephemeral_application",
            "ephemeral_generate",
            "ephemeral_decide",
        ] {
            assert!(
                header.contains(symbol),
                "{symbol} is exported but the header does not declare it"
            );
            assert!(
                source.contains(&format!("pub unsafe extern \"C\" fn {symbol}")),
                "the header declares {symbol}, which the library does not export"
            );
        }

        for status in ["EPHEMERAL_OK", "EPHEMERAL_ERROR", "EPHEMERAL_BAD_HANDLE"] {
            assert!(
                header.contains(status),
                "{status} is missing from the header"
            );
        }
    }
}
