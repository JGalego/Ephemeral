//! # Ephemeral's JNI bridge
//!
//! One adapter, between Android's calling convention and the C ABI in
//! [`ephemeral_ffi`]. It contains no domain logic and makes no decisions: every
//! call here is forwarded to the same C function an iOS application would call,
//! so a phone runs the same lifecycle machine, the same permission ledger and
//! the same audit record as the desktop.
//!
//! ## Why this exists rather than calling C from Kotlin directly
//!
//! Kotlin cannot produce a C function pointer, and
//! [`ephemeral_ffi::ephemeral_open`] needs two of them: Ephemeral opens no
//! sockets on a phone and calls back into the host for every HTTPS round trip
//! ([ADR-0017]). This crate supplies those two pointers and turns each call
//! into an ordinary Java method call on an object the application passed in.
//!
//! ## What crosses the boundary
//!
//! Strings and one opaque `long`. The `long` is a session this crate allocated;
//! it is never a pointer the application can usefully inspect, and passing
//! anything else is rejected rather than dereferenced.
//!
//! Nothing unwinds into a Java frame: a panic across one is undefined
//! behaviour, so every entry point catches it and returns a failure the
//! application can read with `lastError`.
//!
//! [ADR-0017]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md

// JNI decides these names, not this crate: the symbol an application looks up
// is derived from the class and method, and it is camel case.
#![allow(non_snake_case)]

mod bridge;

pub use bridge::*;
