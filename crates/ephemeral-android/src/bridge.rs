//! The JNI entry points, and the two callbacks that make generation possible.
//!
//! Every function here is a forwarder. If you are looking for what a call
//! actually does, it is in `ephemeral-ffi`, and behind that in
//! `ephemeral-core` — the same code the desktop runs.

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;

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
    // A bit-preserving cast, never `try_from`. See `as_handle`.
    let pointer: *mut Session = from_handle(session);
    // SAFETY: non-zero, and by contract it is a pointer `open` returned and
    // `close` has not yet released.
    Some(unsafe { &*pointer })
}

/// Turns a pointer into the number Java holds on to.
///
/// Bit-preserving, and it has to be. Android tags heap pointers in the top
/// byte on arm64 — the hardware ignores those bits when dereferencing, but a
/// signed 64-bit integer does not: a tagged pointer is a *negative* `jlong`,
/// and `i64::try_from` refuses roughly half of every allocation.
///
/// This was `try_from`, and it meant the application could not start on any
/// modern 64-bit phone. It started perfectly on the x86-64 emulator, which
/// does not tag, so nothing caught it until it ran on a Pixel — where the
/// screen said the session did not fit in a Java long, for five minutes,
/// on video.
#[allow(clippy::cast_possible_wrap)]
fn as_handle<T>(pointer: *mut T) -> jlong {
    pointer as usize as jlong
}

/// Turns the number Java holds back into the pointer it was made from.
///
/// The other half of [`as_handle`], and the casts are deliberate in the same
/// way. Clippy objects to both of its worries here, and both are answered:
///
/// - *may lose the sign* — it is meant to. The sign is a tag bit Android put
///   in the top byte, not a negative quantity, and preserving the bits is
///   precisely the job.
/// - *may truncate on 32-bit pointers* — on a 32-bit Android a pointer fits in
///   `usize` with room to spare, so a handle this crate produced always
///   narrows back to exactly what it widened from.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn from_handle<T>(handle: jlong) -> *mut T {
    handle as usize as *mut T
}

/// Runs a body, turning a panic into a failure rather than an unwind into Java.
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

/// Why the last attempt to open failed.
///
/// Every other call reports through the session's own error slot, which is no
/// use for the one call that fails by not producing a session. `open` had six
/// distinct ways to return zero and no way to say which, so a phone that could
/// not start said "Ephemeral could not open its files" and stopped there —
/// which is what a real device did, and it took a code read to even narrow it.
static WHY_OPEN_FAILED: Mutex<Option<String>> = Mutex::new(None);

/// Records why, for `lastError(0)` to hand back.
fn opening_failed(reason: String) -> jlong {
    if let Ok(mut slot) = WHY_OPEN_FAILED.lock() {
        *slot = Some(reason);
    }
    0
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
    headers_json: *const c_char,
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
                CStr::from_ptr(headers_json).to_str(),
                CStr::from_ptr(request_json).to_str(),
            )
        };
        let (Ok(endpoint), Ok(headers), Ok(body)) = borrowed else {
            return ptr::null_mut();
        };

        let Ok(mut guard) = host.vm.attach_current_thread() else {
            return ptr::null_mut();
        };

        let Some(reply) = ask_host(&mut guard, &host.transport, endpoint, headers, body) else {
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
    headers: &str,
    body: &str,
) -> Option<String> {
    let endpoint = env.new_string(endpoint).ok()?;
    let headers = env.new_string(headers).ok()?;
    let body = env.new_string(body).ok()?;

    let outcome = env.call_method(
        transport.as_obj(),
        "send",
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
        &[
            JValue::Object(&endpoint),
            JValue::Object(&headers),
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
    guarded(
        opening_failed("Opening Ephemeral panicked.".to_owned()),
        || {
            let Some(home) = text(&mut env, &home) else {
                return opening_failed(
                    "The path given for Ephemeral's files is unreadable.".to_owned(),
                );
            };
            let Ok(home_c) = CString::new(home.clone()) else {
                return opening_failed(format!("The path {home} cannot be passed to the engine."));
            };
            let home = home_c;
            let (Ok(vm), Ok(transport)) = (env.get_java_vm(), env.new_global_ref(transport)) else {
                return opening_failed(
                    "The Java runtime would not lend a reference to the transport.".to_owned(),
                );
            };

            let host = Box::into_raw(Box::new(Host { vm, transport }));

            // SAFETY: `home` is NUL-terminated, and `host` stays alive until
            // `close` releases it — after the handle it belongs to.
            let ephemeral =
                unsafe { ephemeral_ffi::ephemeral_open(home.as_ptr(), send, release, host.cast()) };

            if ephemeral.is_null() {
                // SAFETY: nothing took ownership of it, so this crate still has it.
                drop(unsafe { Box::from_raw(host) });
                return opening_failed("The engine refused the path it was given.".to_owned());
            }

            let session = Box::into_raw(Box::new(Session { ephemeral, host }));
            as_handle(session)
        },
    )
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
        // Bit-preserving, like `as_handle` — this was `try_from` too, which
        // meant a tagged pointer was not closed but silently leaked, along
        // with the engine handle and the transport's global reference.
        let pointer: *mut Session = from_handle(session);
        // SAFETY: by contract this is a session `open` returned and nobody has
        // closed yet. Taking the box here is what makes a second close a
        // caller error rather than a double free this crate performs.
        let session = unsafe { Box::from_raw(pointer) };
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

/// Chooses which service generates, and how it is configured.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_setProvider<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    configuration: JString<'local>,
) -> jint {
    guarded(EPHEMERAL_ERROR, || {
        let Some(open) = opened(session) else {
            return EPHEMERAL_BAD_HANDLE;
        };
        let Some(configuration) = text(&mut env, &configuration) else {
            return EPHEMERAL_ERROR;
        };
        let Ok(configuration) = CString::new(configuration) else {
            return EPHEMERAL_ERROR;
        };
        // SAFETY: live handle, NUL-terminated JSON borrowed for the call.
        unsafe { ephemeral_ffi::ephemeral_set_provider(open.ephemeral, configuration.as_ptr()) }
    })
}

/// What is currently chosen, as JSON. Carries no credential.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_provider<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle.
        let produced = unsafe { ephemeral_ffi::ephemeral_provider(open.ephemeral) };
        handed_back(&mut env, produced)
    })
}

/// Every provider this build can be pointed at, as JSON.
///
/// Takes no session on purpose: a person may want to choose before there is a
/// workspace, and the answer does not depend on one.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_providers<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        handed_back(&mut env, ephemeral_ffi::ephemeral_providers())
    })
}

/// What the chosen service says it can be asked for, as JSON.
///
/// Reaches the network, through the host's own transport like every other
/// request. It is the connection test: the same endpoint and the same
/// credential generation would use.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_models<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        // SAFETY: live handle.
        let produced = unsafe { ephemeral_ffi::ephemeral_models(open.ephemeral) };
        handed_back(&mut env, produced)
    })
}

/// Turns a filled-in form into the arguments the application receives.
///
/// The phone never composes an argument vector itself: the domain does, so a
/// phone and a terminal cannot disagree about what a filled-in form means.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_arguments<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    id: JString<'local>,
    answers: JString<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        let (Some(id), Some(answers)) = (text(&mut env, &id), text(&mut env, &answers)) else {
            return ptr::null_mut();
        };
        let (Ok(id), Ok(answers)) = (CString::new(id), CString::new(answers)) else {
            return ptr::null_mut();
        };

        // SAFETY: live handle, NUL-terminated strings borrowed for the call.
        let produced = unsafe {
            ephemeral_ffi::ephemeral_arguments(open.ephemeral, id.as_ptr(), answers.as_ptr())
        };
        handed_back(&mut env, produced)
    })
}

/// Runs an application on this device, and says what it did.
///
/// The call that makes a handset something other than a remote control. It
/// blocks for as long as the application runs, which is why nothing in the
/// Kotlin above it may reach this from a thread that draws.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_jgalego_ephemeral_Native_run<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    session: jlong,
    id: JString<'local>,
    arguments: JString<'local>,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let Some(open) = opened(session) else {
            return ptr::null_mut();
        };
        let (Some(id), Some(arguments)) = (text(&mut env, &id), text(&mut env, &arguments)) else {
            return ptr::null_mut();
        };
        let (Ok(id), Ok(arguments)) = (CString::new(id), CString::new(arguments)) else {
            return ptr::null_mut();
        };

        // SAFETY: live handle, NUL-terminated strings borrowed for the call.
        let produced = unsafe {
            ephemeral_ffi::ephemeral_run(open.ephemeral, id.as_ptr(), arguments.as_ptr())
        };
        handed_back(&mut env, produced)
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
            // No session, so the question can only be about the attempt to make
            // one. Answering "nothing went wrong" here is how a phone that
            // could not start came to say nothing about why.
            let said = WHY_OPEN_FAILED.lock().ok().and_then(|slot| slot.clone());
            return match said {
                Some(reason) => env
                    .new_string(reason)
                    .map_or(ptr::null_mut(), jni::objects::JString::into_raw),
                None => ptr::null_mut(),
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Android tags heap pointers in the top byte on arm64. The hardware
    /// ignores those bits when dereferencing; `i64::try_from` does not, and
    /// refuses every allocation whose tag sets the high bit.
    ///
    /// This is what shipped: the application started on the x86-64 emulator,
    /// where nothing is tagged, and could not start on any modern phone. Five
    /// minutes of video of a Pixel 8 saying "The session does not fit in a
    /// Java long" is what found it. A handle is a bit pattern, not a number,
    /// and this asserts it round-trips as one.
    #[test]
    fn a_tagged_pointer_survives_the_trip_through_java() {
        // What Android's allocator hands out: a real address with a tag in the
        // top byte, which as a signed 64-bit integer is negative.
        let tagged = 0xb400_007a_1b2c_3d40_usize as *mut u8;

        let handle = as_handle(tagged);
        assert!(handle < 0, "a tagged pointer is a negative jlong");
        assert_eq!(
            from_handle::<u8>(handle),
            tagged,
            "and it has to come back as the same address"
        );

        // The refusal this replaced.
        assert!(
            jlong::try_from(tagged as usize).is_err(),
            "which is exactly what try_from would not accept"
        );
    }

    /// The untagged case still works, so the fix is not a trade.
    #[test]
    fn an_ordinary_pointer_survives_it_too() {
        let plain = 0x0000_7f2a_9c31_0000_usize as *mut u8;

        assert!(as_handle(plain) > 0);
        assert_eq!(from_handle::<u8>(as_handle(plain)), plain);
    }

    /// Zero is the documented "no session", and must stay distinguishable from
    /// a real handle in both directions.
    #[test]
    fn zero_is_never_a_session() {
        assert!(opened(0).is_none());
        assert_eq!(as_handle(std::ptr::null_mut::<u8>()), 0);
    }
}
