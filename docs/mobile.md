# Ephemeral on a phone

Ephemeral's engine runs on iOS and Android.

**On Android there is an application.** It is built from
[`apps/android`](../apps/android), it ships as an APK on the
[releases page](https://github.com/JGalego/Ephemeral/releases), and its own
README covers installing and building it. Read that first if you want to *use*
Ephemeral on a phone.

**On iOS there is not, yet.** What is published there is the engine — a static
library and a C header — and there is something to build an application with
rather than something to install.

This page is for whoever writes that application, on either platform. It
describes the boundary both sides use; the Android app is one implementation of
it, and a readable example of what the rest of this page is asking for.

## What it does on a device, and what it does not

**It generates.** A sentence becomes an application: Ephemeral plans it, asks a
model for the source, and writes that source to the device. The permission
requests the application makes are recorded, and a person answers them.

**It does not build or run what it generated.** That needs a sandbox, and a
phone does not have one that a third-party application may use. Running
generated code outside a sandbox is the specific thing Ephemeral exists to
prevent, so it is not done unsandboxed on a phone as a convenience.

An application generated on a phone is therefore real source on the device that
nothing there can execute. That is a deliberate stop, not an unfinished feature,
and **your interface has to say so** — somebody who taps "create" and is left
watching a spinner for a build that will never happen has been misled by your
app, not by this library. A desktop or a control plane
([ADR-0007](architecture/decisions/0007-mobile-control-plane.md)) finishes the
job.

The reasoning is in
[ADR-0017](architecture/decisions/0017-mobile-generates-through-a-host-transport.md).

## What you get

| Platform | Artifact | Contains |
|---|---|---|
| iOS | `Ephemeral-<version>-ios.xcframework.zip` | Device and simulator slices, `ephemeral.h`, and a module map |
| Android | `ephemeral-<version>-android.tar.gz` | `jniLibs/{arm64-v8a,armeabi-v7a,x86_64,x86}/libephemeral_ffi.a` and `include/ephemeral.h` |
| Android | `ephemeral-<version>.apk` | The application, with the engine already in it |

The APK is for installing. The tarball is for building something else with — it
carries the C ABI, not the JNI bridge, so a Kotlin caller wanting the shortcut
should look at `crates/ephemeral-android` instead.

Both are on the [releases page](https://github.com/JGalego/Ephemeral/releases).
`ephemeral.h` is the specification: it documents ownership, threading and the
failure contract, and it is the file to read before this one.

## You supply the network

Ephemeral opens no sockets on a phone and brings no HTTP stack. You pass it two
function pointers — one that performs a request, one that frees the response —
and it calls them whenever it needs the model.

This is not a limitation worked around; it is the design. TLS policy,
certificate pinning, background transfer and proxy behaviour stay with the
platform code that is allowed to have opinions about them, and your app does not
carry a second TLS stack. It is also what makes generating on iOS possible at
all: the desktop transport spawns `curl`, and an iOS application may not spawn a
process.

Your send function receives an endpoint, a header set and a JSON body. POST the
body to the endpoint with **exactly** those headers and nothing else:

```json
[{"name": "x-api-key",         "value": "…"},
 {"name": "anthropic-version", "value": "2023-06-01"},
 {"name": "content-type",      "value": "application/json"}]
```

Which headers a service wants is the provider's knowledge. This used to be a
bare `api_key` that you wrapped in Anthropic's three yourself, which meant the
shape of this callback decided that a phone talks to Anthropic — see
[ADR-0020](architecture/decisions/0020-the-host-chooses-the-provider.md).

Return the response body as a newly allocated NUL-terminated string, or `NULL`
on any failure. Ephemeral copies it immediately and then hands it to your free
function, so the allocation is yours from beginning to end.

## Which service

`ephemeral_set_provider` takes `{"provider":…}` with optional `base_url`,
`model` and `ceiling`; `ephemeral_providers` lists what this build has, what
each needs and what each defaults to. Build your picker from that call rather
than from a list in your own source: your application ships on its own schedule,
and a hardcoded list of providers is wrong the moment one is added.

A name that is not in the catalogue is refused rather than defaulted past.
Generating with a company somebody did not choose is worse than not generating.

## The credential

Pass it in with `ephemeral_set_credential`, from Keychain on iOS or Keystore on
Android. **The library does not read an environment variable and will not look
for one** — `ANTHROPIC_API_KEY` is a desktop convention, and a phone has no
equivalent.

It is separate from the provider choice on purpose: the choice is ordinary
preferences and can be read back with `ephemeral_provider`, and no credential
appears in that answer because none was ever part of it.

It lives in memory for the duration of a call. Do not write it to a file, a
preference, or a log.

## Swift

There is a whole application now, in [`apps/ios`](../apps/ios) — the engine
wrapper, the transport, the Keychain and every screen — and CI type-checks it
against the iOS SDK on every commit. What follows is the shape of it, kept here
because the transport is the part somebody embedding the engine in their own
application has to write themselves.

Add the XCFramework to your target and `import Ephemeral`.

```swift
import Ephemeral
import Foundation

final class EphemeralEngine {
    private let handle: OpaquePointer

    init(home: URL, credential: String) throws {
        // Two C function pointers. Neither may capture context, so everything
        // they need travels through the `context` argument instead.
        let send: EphemeralHttpSend = { context, endpoint, headersJson, body in
            guard let endpoint, let headersJson, let body,
                  let url = URL(string: String(cString: endpoint))
            else { return nil }

            var request = URLRequest(url: url)
            request.httpMethod = "POST"
            request.httpBody = Data(String(cString: body).utf8)

            // Exactly what the provider composed, and nothing added here.
            let headers = try? JSONSerialization.jsonObject(
                with: Data(String(cString: headersJson).utf8)
            )
            for header in headers as? [[String: String]] ?? [] {
                guard let name = header["name"], let value = header["value"] else { continue }
                request.setValue(value, forHTTPHeaderField: name)
            }

            // Ephemeral's own call is synchronous, so this waits. Call
            // `generate` off the main thread — it is a model request and takes
            // as long as one takes.
            let semaphore = DispatchSemaphore(value: 0)
            var reply: String?
            URLSession.shared.dataTask(with: request) { data, _, _ in
                reply = data.flatMap { String(data: $0, encoding: .utf8) }
                semaphore.signal()
            }.resume()
            semaphore.wait()

            // strdup, because Ephemeral frees this through our free function.
            return reply.map { strdup($0) } ?? nil
        }

        let free: EphemeralHttpFree = { _, response in
            Foundation.free(response)
        }

        guard let opened = ephemeral_open(home.path, send, free, nil) else {
            throw EngineError.couldNotOpen
        }
        self.handle = opened

        guard ephemeral_set_credential(handle, credential) == EPHEMERAL_OK else {
            throw EngineError.credentialRejected
        }
    }

    deinit { ephemeral_close(handle) }

    /// Every string this library returns is ours to release.
    private func take(_ owned: UnsafeMutablePointer<CChar>?) -> String? {
        guard let owned else { return nil }
        defer { ephemeral_string_free(owned) }
        return String(cString: owned)
    }

    private var lastError: String {
        take(ephemeral_last_error(handle)) ?? "no reason given"
    }

    func create(intent: String) throws -> Data {
        guard let json = take(ephemeral_create(handle, intent)) else {
            throw EngineError.refused(lastError)
        }
        return Data(json.utf8)
    }

    /// Plans, generates and writes source to the device. Not on the main thread.
    func generate(id: String) throws -> Data {
        guard let json = take(ephemeral_generate(handle, id)) else {
            throw EngineError.refused(lastError)
        }
        return Data(json.utf8)
    }

    /// Records a person's answer. `capability` must be one this application
    /// actually asked for — anything else is refused rather than granted.
    func decide(id: String, capability: String, allow: Bool) throws {
        guard ephemeral_decide(handle, id, capability, allow) == EPHEMERAL_OK else {
            throw EngineError.refused(lastError)
        }
    }

    enum EngineError: Error {
        case couldNotOpen
        case credentialRejected
        case refused(String)
    }
}
```

Use an Application Support directory for `home`, excluded from iCloud backup.

## Kotlin

Kotlin reaches C through JNI or the Panama FFI. Either way the shape is the
same as above: open with two function pointers, keep the handle, free every
returned string.

Put the archives under `src/main/jniLibs/<abi>/` — which is the layout the
release tarball already unpacks into — and link `libephemeral_ffi.a` from a
small JNI shim built by your `CMakeLists.txt`. `home` should be app-private
internal storage, and the credential should come from Keystore or
`EncryptedSharedPreferences`.

## Rules that are not suggestions

**Free what you are given.** Every `char *` the library returns is yours, and
`ephemeral_string_free` is the only correct way to release it. Every `const
char *` you pass in is borrowed for the duration of the call and not retained.

**One thread at a time per handle.** The handle is not a lock. If your interface
can start two operations at once, serialise them.

**Nothing unwinds into your frame.** Every entry point catches. A failure is a
`NULL` or a non-zero code, and `ephemeral_last_error` says why in words meant
for a person — show them rather than an error number.

**Generating grants nothing.** `ephemeral_generate` records what an application
asked for. Only `ephemeral_decide` grants anything, and a capability nobody
requested cannot be granted through this ABI at all: it is refused, not composed
out of the string you passed. Your interface cannot accidentally widen what an
application may do, and it should not try to.

## What is not written yet

The Swift and Kotlin applications themselves. The engine is compiled against
from C on every commit and built for five device architectures, so what is left
is a user interface rather than a contract — but it is genuinely left, and this
page describes a library rather than an app that exists.

## What a permission decision means on a phone

Both halves of the permission model reach the device
([ADR-0003](architecture/decisions/0003-two-tier-permission-model.md)): an
application may do something only if the person allowed *it* and allowed
*Ephemeral* to carry it out. Nothing on a phone mirrors the operating system's
own permissions into the ledger yet — that is the platform adapter, and it does
not exist — so allowing an application something here records the decision and
grants no authority. The page says so outright rather than by omission: the
capability comes back with `"effective": false` and what Ephemeral is missing.

That costs nothing today, because a phone generates and does not run. It is what
the adapter has to fix before one does, and the C ABI test asserts the current
behaviour so that fixing it is a visible change rather than a quiet one.
