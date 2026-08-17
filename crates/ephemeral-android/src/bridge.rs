//! The JNI entry points, and the two callbacks that make generation possible.
//!
//! Every function here is a forwarder. If you are looking for what a call
//! actually does, it is in `ephemeral-ffi`, and behind that in
//! `ephemeral-core` — the same code the desktop runs.

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};

use ephemeral_ffi::{EPHEMERAL_BAD_HANDLE, EPHEMERAL_ERROR, Ephemeral};

/// One open Ephemeral, plus the Java object it calls back into.
///
/// The application holds this as a `long` and nothing else. Both pointers are
/// owned here and released together by `close`, which matters because the C ABI
/// keeps the host context for as long as the handle lives: freeing the callback
/// object first would leave Ephemeral holding a dangling context.
struct Session {
    ephemeral: *mut Ephemeral,
    host: *mut Host,
}

/// What the transport callbacks need to reach Java from whatever thread
/// Ephemeral happens to be on.
struct Host {
    vm: JavaVM,
    transport: GlobalRef,
}

/// Reads a Java string, or `None` if it was null or not readable.
fn text(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Option<String> {
    env.get_string(value).ok().map(Into::into)
}

/// Turns a string Ephemeral produced into a Java string, releasing the original.
///
/// The C ABI hands back an allocation this crate must free with
/// [`ephemeral_ffi::ephemeral_string_free`]; copying first and freeing
/// immediately means there is no path out of here that leaks it.
#[allow(unsafe_code)]
fn handed_back(env: &mut JNIEnv<'_>, produced: *mut c_char) -> jstring {
    if produced.is_null() {
        return ptr::null_mut();
    }

    // SAFETY: non-null, and by the C ABI's documented contract it is a
    // NUL-terminated string owned by the caller until it is freed below.
    let copied = unsafe { CStr::from_ptr(produced) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the pointer came from this library and has not been freed.
    unsafe { ephemeral_ffi::ephemeral_string_free(produced) };

    env.new_string(copied)
        .map_or(ptr::null_mut(), jni::objects::JString::into_raw)
}

/// Borrows an open session, or `None` if the application passed something else.
///
/// A zero is the documented "no session" value and is the only invalid input
/// that can be recognised: any other number is taken at its word, which is the
/// same contract every opaque-handle API has.
#[allow(unsafe_code)]
fn opened<'a>(session: jlong) -> Option<&'a Session> {
    if session == 0 {
        return None;
    }
    let pointer = usize::try_from(session).ok()? as *const Session;
    // SAFETY: non-zero, and by contract it is a pointer `open` returned and
    // `close` has not yet released.
    Some(unsafe { &*pointer })
}

/// Runs a body, turning a panic into a failure rather than an unwind into Java.
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

// ---------------------------------------------------------------------------
// The transport callbacks
// ---------------------------------------------------------------------------

/// Performs one HTTPS round trip by calling back into the application.
///
/// Matches [`ephemeral_ffi::EphemeralHttpSend`]. Returns null on any failure,
/// including a Java exception, which is cleared here rather than left pending —
/// returning into native code with an exception in flight is undefined.
#[allow(unsafe_code)]
extern "C" fn send(
    context: *mut c_void,
    endpoint: *const c_char,
    api_key: *const c_char,
    request_json: *const c_char,
) -> *mut c_char {
    guarded(ptr::null_mut(), || {
        if context.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: the context is the `Host` this crate handed to `ephemeral_open`,
        // and it outlives the handle by construction — see `Session`.
        let host = unsafe { &*context.cast::<Host>() };

        // SAFETY: the C ABI guarantees three NUL-terminated strings valid for
        // the duration of this call.
        let borrowed = unsafe {
            (
                CStr::from_ptr(endpoint).to_str(),
                CStr::from_ptr(api_key).to_str(),
                CStr::from_ptr(request_json).to_str(),
            )
        };
        let (Ok(endpoint), Ok(api_key), Ok(body)) = borrowed else {
            return ptr::null_mut();
        };

        let Ok(mut guard) = host.vm.attach_current_thread() else {
            return ptr::null_mut();
        };

        let Some(reply) = ask_host(&mut guard, &host.transport, endpoint, api_key, body) else {
            return ptr::null_mut();
        };

        CString::new(reply).map_or(ptr::null_mut(), CString::into_raw)
    })
}

/// The Java side of [`send`], separated so every failure is one `?`.
fn ask_host(
    env: &mut JNIEnv<'_>,
    transport: &GlobalRef,
    endpoint: &str,
    api_key: &str,
    body: &str,
) -> Option<String> {
    let endpoint = env.new_string(endpoint).ok()?;
    let api_key = env.new_string(api_key).ok()?;
    let body = env.new_string(body).ok()?;

    let outcome = env.call_method(
        transport.as_obj(),
        "send",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        &[
            JValue::Object(&endpoint),
            JValue::Object(&api_key),
            JValue::Object(&body),
        ],
    );

    // Clear before anything else: a pending exception poisons every later JNI
    // call, so a failure to send would turn into a crash somewhere unrelated.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        return None;
    }

    let answer = outcome.ok()?.l().ok()?;
    if answer.is_null() {
        return None;
    }
    env.get_string(&JString::from(answer)).ok().map(Into::into)
}

/// Releases a response [`send`] returned. Matches [`ephemeral_ffi::EphemeralHttpFree`].
#[allow(unsafe_code)]
extern "C" fn release(_context: *mut c_void, response: *mut c_char) {
    if response.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: this pointer was produced by `CString::into_raw` in `send`
        // and is handed back exactly once, which the C ABI documents.
        drop(unsafe { CString::from_raw(response) });
    });
}

// ---------------------------------------------------------------------------
// The entry points
// ---------------------------------------------------------------------------

/// Opens Ephemeral under `home`, calling back into `transport` for HTTPS.
///
/// Returns 0 on failure. `transport` must have a
/// `String send(String, String, String)` method; it is held as a global
/// reference until `close`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_open<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    home: JString<'local>,
    transport: JObject<'local>,
) -> jlong {
    guarded(0, || {
        let Some(home) = text(&mut env, &home) else {
            return 0;
        };
        let Ok(home) = CString::new(home) else {
            return 0;
        };
        let (Ok(vm), Ok(transport)) = (env.get_java_vm(), env.new_global_ref(transport)) else {
            return 0;
        };

        let host = Box::into_raw(Box::new(Host { vm, transport }));

        // SAFETY: `home` is NUL-terminated, and `host` stays alive until
        // `close` releases it — after the handle it belongs to.
        let ephemeral =
            unsafe { ephemeral_ffi::ephemeral_open(home.as_ptr(), send, release, host.cast()) };

        if ephemeral.is_null() {
            // SAFETY: nothing took ownership of it, so this crate still has it.
            drop(unsafe { Box::from_raw(host) });
            return 0;
        }

        let session = Box::into_raw(Box::new(Session { ephemeral, host }));
        jlong::try_from(session as usize).unwrap_or(0)
    })
}

/// Closes a session. Passing 0 is allowed and does nothing.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_close(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    session: jlong,
) {
    if session == 0 {
        return;
    }
    guarded((), || {
        let Ok(pointer) = usize::try_from(session) else {
            return;
        };
        // SAFETY: by contract this is a session `open` returned and nobody has
        // closed yet. Taking the box here is what makes a second close a
        // caller error rather than a double free this crate performs.
        let session = unsafe { Box::from_raw(pointer as *mut Session) };
        // SAFETY: the handle is live and was produced by `ephemeral_open`.
        unsafe { ephemeral_ffi::ephemeral_close(session.ephemeral) };
        // SAFETY: released after the handle, which held the context.
        drop(unsafe { Box::from_raw(session.host) });
    });
}

/// Supplies the model credential, from the platform's secure store.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_setCredential<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    api_key: JString<'local>,
) -> jint {
    guarded(EPHEMERAL_ERROR, || {
        let Some(open) = opened(session) else {
            return EPHEMERAL_BAD_HANDLE;
        };
        let Some(api_key) = text(&mut env, &api_key) else {
            return EPHEMERAL_ERROR;
        };
        let Ok(api_key) = CString::new(api_key) else {
            return EPHEMERAL_ERROR;
        };
        // SAFETY: live handle, NUL-terminated credential borrowed for the call.
        unsafe { ephemeral_ffi::ephemeral_set_credential(open.ephemeral, api_key.as_ptr()) }
    })
}

/// Why the last call failed, or null.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_lastError<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle.
        let produced = unsafe { ephemeral_ffi::ephemeral_last_error(open.ephemeral) };
        handed_back(&mut env, produced)
    })
}

/// Records a new application from a sentence. Needs no credential and no network.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_create<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    intent: JString<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        let Some(intent) = text(&mut env, &intent) else {
            return ptr::null_mut();
        };
        let Ok(intent) = CString::new(intent) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle, NUL-terminated intent borrowed for the call.
        let produced = unsafe { ephemeral_ffi::ephemeral_create(open.ephemeral, intent.as_ptr()) };
        handed_back(&mut env, produced)
    })
}

/// Every application, most recently touched first, as a JSON array.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_applications<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle.
        let produced = unsafe { ephemeral_ffi::ephemeral_applications(open.ephemeral) };
        handed_back(&mut env, produced)
    })
}

/// One application's page, as JSON.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_application<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    id: JString<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        let Some(id) = text(&mut env, &id) else {
            return ptr::null_mut();
        };
        let Ok(id) = CString::new(id) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle, NUL-terminated id borrowed for the call.
        let produced = unsafe { ephemeral_ffi::ephemeral_application(open.ephemeral, id.as_ptr()) };
        handed_back(&mut env, produced)
    })
}

/// Plans and generates an application, writing its source to the device.
///
/// Blocking, and it calls back into the transport: call it off the main thread.
/// It deliberately does not build or run what it generated — a phone has no
/// sandbox, and running generated code without one is the thing Ephemeral
/// exists to prevent.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_generate<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    id: JString<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        let Some(id) = text(&mut env, &id) else {
            return ptr::null_mut();
        };
        let Ok(id) = CString::new(id) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle, NUL-terminated id borrowed for the call.
        let produced = unsafe { ephemeral_ffi::ephemeral_generate(open.ephemeral, id.as_ptr()) };
        handed_back(&mut env, produced)
    })
}

/// Records a person's answer to one thing an application asked for.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_decide<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    id: JString<'local>,
    capability: JString<'local>,
    allow: jboolean,
) -> jint {
    guarded(EPHEMERAL_ERROR, || {
        let Some(open) = opened(session) else {
            return EPHEMERAL_BAD_HANDLE;
        };
        let (Some(id), Some(capability)) = (text(&mut env, &id), text(&mut env, &capability))
        else {
            return EPHEMERAL_ERROR;
        };
        let (Ok(id), Ok(capability)) = (CString::new(id), CString::new(capability)) else {
            return EPHEMERAL_ERROR;
        };
        // SAFETY: live handle, both strings NUL-terminated and borrowed for the call.
        unsafe {
            ephemeral_ffi::ephemeral_decide(
                open.ephemeral,
                id.as_ptr(),
                capability.as_ptr(),
                allow != 0,
            )
        }
    })
}
