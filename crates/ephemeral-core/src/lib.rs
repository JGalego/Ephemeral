//! # Ephemeral core
//!
//! The domain model for Ephemeral 🫧 — the part of the system that decides what
//! an application *is*, what it is *allowed to do*, and what state it is *in*.
//!
//! This crate is deliberately the boring, portable, heavily-tested centre of the
//! product. It is compiled unchanged for macOS, Windows, Linux, iOS and Android
//! so that a permission decision cannot mean one thing on a laptop and another
//! on a phone ([ADR-0002]).
//!
//! ## What lives here
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`lifecycle`] | The explicit state machine every application moves through |
//! | [`permission`] | Both permission systems, and the ledger that decides |
//! | [`identity`] | Application ids and the principals that hold permissions |
//! | [`actor`] | Who caused something to happen |
//!
//! Landing next, in Phase 0: the versioned application manifest, retention
//! policies, the audit log and the storage layout.
//!
//! ## What does not live here
//!
//! No Docker, no network, no platform APIs, no model providers. With the `fs`
//! feature disabled this crate performs no host I/O whatsoever, and CI builds it
//! that way to keep the boundary honest. Anything that touches the host lives in
//! a runtime, platform-adapter or agent crate on the far side of a trait.
//!
//! ## The invariants this crate exists to hold
//!
//! 1. A generated application inherits **nothing** from Ephemeral's own
//!    permissions ([`permission`]).
//! 2. Permission checks are **default-deny**, and an explicit denial wins.
//! 3. Lifecycle transitions are **total and explicit** — an illegal transition is
//!    a typed error, never a silent state change ([`lifecycle`]).
//! 4. The generation agent is **not a privileged actor**: it cannot grant
//!    permissions or delete applications ([`actor`]).
//! 5. Secrets never enter a manifest, a log, or the audit record.
//!
//! Each of these is covered by tests in `tests/`, and a change that weakens one
//! should be treated as a vulnerability rather than a refactor.
//!
//! [ADR-0002]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0002-rust-core-with-platform-shells.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod actor;
pub mod error;
pub mod identity;
pub mod lifecycle;
pub mod permission;

pub use actor::Actor;
pub use error::{Error, Result};
pub use identity::{AppId, PluginId, Principal};
pub use lifecycle::{Lifecycle, LifecycleEvent, LifecycleState, Transition};
pub use permission::{AppPermission, Decision, Grant, MetaPermission, PermissionLedger};

/// A point in time, always in UTC.
///
/// Every timestamp Ephemeral records — lifecycle transitions, grants, audit
/// entries — uses this type. UTC rather than local time because these records
/// are compared across machines, exported, and read long after the fact.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// The current time.
///
/// Centralised so that tests and the (future) replay tooling have one place to
/// intercept.
#[must_use]
pub fn now() -> Timestamp {
    chrono::Utc::now()
}
