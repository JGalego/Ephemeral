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
 * POST `request_json` to `endpoint` with exactly the headers in
 * `headers_json`, which is a JSON array in the order the provider composed
 * them:
 *
 *     [{"name": "x-api-key", "value": "…"},
 *      {"name": "anthropic-version", "value": "2023-06-01"},
 *      {"name": "content-type", "value": "application/json"}]
 *
 * Set those and add nothing. The credential is one of them, and which headers
 * a service wants is the provider's business — this used to be a single
 * `api_key` that you wrapped in Anthropic's headers yourself, which meant a
 * phone could only ever talk to Anthropic however it was configured.
 *
 * Return the response body as a newly allocated NUL-terminated string, or NULL
 * on any failure. Ephemeral copies it immediately and then passes it to your
 * free function, so the allocation is yours throughout.
 */
typedef char *(*EphemeralHttpSend)(void *context, const char *endpoint,
                                   const char *headers_json,
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

/*
 * Chooses which service generates, and how it is configured.
 *
 * `configuration_json` is:
 *
 *     {"provider": "openai",
 *      "base_url": "https://api.groq.com/openai/v1",
 *      "model":    "llama-3.3-70b-versatile",
 *      "ceiling":  "max_completion_tokens"}
 *
 * Only `provider` is required; everything absent means that provider's own
 * default. A name that is not in `ephemeral_providers` is refused rather than
 * defaulted past — generating with a company somebody did not choose is worse
 * than not generating.
 *
 * The credential is not part of this and does not belong in it: it comes from
 * the secure store through `ephemeral_set_credential`, which is what lets this
 * be saved in ordinary preferences.
 */
int32_t ephemeral_set_provider(EphemeralHandle *handle,
                               const char *configuration_json);

/*
 * What is currently chosen, as the same JSON `ephemeral_set_provider` takes.
 * Carries no credential, because a credential was never part of it.
 */
char *ephemeral_provider(EphemeralHandle *handle);

/*
 * Turns a filled-in form into the arguments the application receives.
 *
 * `answers_json` is {"input name": "what somebody typed", ...}; the result is a
 * JSON array of strings. NULL on refusal, with the reason in
 * ephemeral_last_error in words meant for the person who filled the form in.
 *
 * You could build this yourself from the `inputs` on the application's page.
 * Do not: a phone, a window and a terminal composing argument vectors
 * separately are three subtly different applications, and the one that gets a
 * flag's default wrong sends a program the opposite of what somebody chose.
 */
char *ephemeral_arguments(EphemeralHandle *handle, const char *id,
                          const char *answers_json);

/*
 * What the chosen service says it can be asked for:
 *
 *     [{"id": "openai/gpt-oss-120b", "name": "GPT OSS 120B", "ceiling": 65536}, …]
 *
 * This is the connection test as well as the model list, because they have one
 * answer. It calls your send function against the endpoint and credential
 * generation would use, so a wrong key, a base URL pointing at nothing, or a
 * retired model all surface here rather than in the middle of a generation
 * somebody is paying for.
 *
 * `ceiling` is the largest reply that model will accept a request for, when the
 * service publishes it. Worth showing: a model with a 16k window refuses a
 * request for more, with a message about a field the person never typed.
 *
 * NULL on failure, with the service's own words in ephemeral_last_error.
 */
char *ephemeral_models(EphemeralHandle *handle);

/*
 * Every provider this build can be pointed at, as a JSON array:
 *
 *     [{"name": "openai", "what": "…", "needs_credential": true,
 *       "configurable": ["base_url", "model", "ceiling"],
 *       "base_url": "https://api.openai.com/v1", "model": "gpt-5"}, …]
 *
 * Build your picker from this rather than from a list of your own: your
 * application ships on its own schedule, and a hardcoded list is wrong the
 * moment a provider is added. Needs no handle.
 */
char *ephemeral_providers(void);

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
