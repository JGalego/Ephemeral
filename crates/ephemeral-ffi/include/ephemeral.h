/*
 * Ephemeral's C ABI — what iOS and Android link against.
 *
 * Hand-written rather than generated, so it can say what the contract is
 * instead of only what the types are. It is checked against the Rust side by a
 * test in `src/lib.rs`, which fails if a symbol here is not exported.
 *
 * Threading: an `EphemeralHandle` is safe to use from one thread at a time.
 * Ephemeral never dereferences your `context`; it only hands it back to your
 * own callbacks.
 *
 * Ownership: every `char *` this library returns is yours, and you release it
 * with `ephemeral_string_free`. Every `const char *` you pass in is borrowed
 * only for the duration of the call.
 *
 * Failure: functions returning a string return NULL on failure; functions
 * returning `int32_t` return a non-zero code. Either way,
 * `ephemeral_last_error` says what went wrong, in words meant for a person.
 * Nothing here unwinds into your frame.
 */

#ifndef EPHEMERAL_H
#define EPHEMERAL_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EPHEMERAL_OK 0
#define EPHEMERAL_ERROR -1
#define EPHEMERAL_BAD_HANDLE -2

/* One open Ephemeral. Opaque: hold the pointer, nothing else. */
typedef struct Ephemeral EphemeralHandle;

/*
 * Sends one HTTPS request. You implement this — URLSession on iOS, whatever
 * you already use on Android.
 *
 * Ephemeral does not open sockets and does not bring an HTTP stack, so TLS,
 * certificate pinning and background-transfer policy stay yours. This is also
 * what makes generating on iOS possible at all: the desktop transport spawns
 * `curl`, and iOS does not allow a process to spawn another process.
 *
 * POST `request_json` to `endpoint` with these headers:
 *
 *     x-api-key: <api_key>
 *     anthropic-version: 2023-06-01
 *     content-type: application/json
 *
 * Return the response body as a newly allocated NUL-terminated string, or NULL
 * on any failure. Ephemeral copies it immediately and then passes it to your
 * free function, so the allocation is yours throughout.
 */
typedef char *(*EphemeralHttpSend)(void *context, const char *endpoint,
                                   const char *api_key,
                                   const char *request_json);

/* Releases a response your send function returned. */
typedef void (*EphemeralHttpFree)(void *context, char *response);

/*
 * Opens Ephemeral, storing its files under `home`.
 *
 * On iOS that should be an Application Support directory excluded from iCloud
 * backup; on Android, app-private internal storage. Returns NULL on failure.
 */
EphemeralHandle *ephemeral_open(const char *home, EphemeralHttpSend send,
                                EphemeralHttpFree free_response, void *context);

/*
 * Supplies the model credential, from the platform's secure store — Keychain
 * on iOS, Keystore/EncryptedSharedPreferences on Android.
 *
 * Never read from an environment variable: that is a desktop convention, and
 * this library does not look for one.
 */
int32_t ephemeral_set_credential(EphemeralHandle *handle, const char *api_key);

/* Closes Ephemeral. Passing NULL is allowed and does nothing. */
void ephemeral_close(EphemeralHandle *handle);

/* Why the last call failed, or NULL. Free it with ephemeral_string_free. */
char *ephemeral_last_error(EphemeralHandle *handle);

/* Releases a string this library returned. NULL is allowed. */
void ephemeral_string_free(char *text);

/*
 * Records a new application from a sentence. Returns its summary as JSON.
 * Needs no credential and no network.
 */
char *ephemeral_create(EphemeralHandle *handle, const char *intent);

/* Every application, most recently touched first, as a JSON array. */
char *ephemeral_applications(EphemeralHandle *handle);

/* One application's page, as JSON. */
char *ephemeral_application(EphemeralHandle *handle, const char *id);

/*
 * Plans and generates an application, writing its source to the device.
 *
 * Deliberately does NOT build, run or test it: that needs a sandbox no phone
 * has, and running generated code outside one is the thing Ephemeral exists to
 * prevent. The application is left generated-and-unbuilt, which is a state the
 * lifecycle already models; a machine that can build finishes it.
 *
 * Calls your send function. Returns the application's page as JSON.
 */
char *ephemeral_generate(EphemeralHandle *handle, const char *id);

/*
 * Records a person's answer to one thing an application asked for.
 *
 * `capability` must be one the application actually requested — a permission
 * cannot be composed out of a string here, so this cannot grant something
 * nobody asked for.
 */
int32_t ephemeral_decide(EphemeralHandle *handle, const char *id,
                         const char *capability, bool allow);

#ifdef __cplusplus
}
#endif

#endif /* EPHEMERAL_H */
