//! # Ephemeral engine
//!
//! The operations that need *this machine*: a container runtime to build and
//! run an application, and a model provider to write one.
//!
//! ## Why this is not in `ephemeral-api`
//!
//! [`ephemeral-api`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-api)
//! is the service layer every client shares, and it holds no I/O of its own on
//! purpose: it compiles for a phone, where there is no daemon and no
//! subprocess. Generating talks to a model and running needs a container, so
//! neither belongs there.
//!
//! For as long as there was one client with a daemon, that left them in the
//! terminal — which was fine until a second client wanted to run an application
//! too. A window with its own copy of "plan, generate, build, repair, record"
//! would be the second, subtly different Ephemeral that the service layer
//! exists to prevent, and the difference would show up as two applications with
//! the same name behaving differently depending on which client started them.
//!
//! So: **`ephemeral-api` is what every client can do; this is what a client
//! with a machine underneath it can do.** The CLI and the desktop window both
//! call this, and neither sequences these steps itself.
//!
//! ## What comes out of it
//!
//! Data, and sentences already phrased for a person — the same rule the views
//! follow. Nothing here prints, formats for a terminal, or knows what a button
//! is; a client decides how to draw what it is told. Where an operation has
//! something a person needs to know — that a capability they granted is doing
//! nothing, that the paths to pass are not the paths on this machine — the
//! sentence comes from here so that both clients say it the same way.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod container;
pub mod generation;
pub mod sandbox;

pub use container::{
    Orphan, Reconciled, Started, Sweep, orphans, output, pause, reconcile, remove_orphans, resume,
    start, stop, sweep,
};
pub use generation::{Generated, PROVIDERS, Requested, generate, provider_authority};
pub use sandbox::{Confinement, specification};
