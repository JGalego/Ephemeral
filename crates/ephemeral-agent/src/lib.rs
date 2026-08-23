//! # Ephemeral agent
//!
//! The boundary between Ephemeral and whatever writes the code.
//!
//! Everything a model produces arrives through this crate, and the crate exists
//! to make one thing structurally true: **a model's output is a request, not a
//! command** ([ADR-0008]). Nothing here can grant a permission, widen a limit,
//! or move an application through its lifecycle. A provider returns typed
//! proposals; the caller decides what to do with them, and the state machine
//! and the permission ledger decide whether it may.
//!
//! ## What lives here
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`provider`] | The [`AgentProvider`] trait every model implementation satisfies |
//! | [`plan`] | What a model proposes an application should be |
//! | [`build`] | The bounded plan → generate → build → repair loop |
//! | [`mock`] | A deterministic provider, which is what CI runs against |
//!
//! ## Why the mock is not a testing convenience
//!
//! CI makes no live model call, ever. A test that needs one is
//! non-deterministic, costs money, needs a credential and needs the network —
//! and the build-and-repair loop, which is the most intricate control flow in
//! the product, is exactly the thing that most needs testing. [`mock`] is
//! therefore a first-class implementation rather than a stub, and the loop is
//! tested through it end to end.
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod build;
pub mod dialogue;
pub mod mock;
pub mod plan;
pub mod provider;
pub mod transport;

pub use build::{Builder, Cancellation, Elapsed, Outcome, RealClock, Round, Run, generate};
pub use mock::MockProvider;
pub use plan::{GeneratedApp, Plan, RepairAttempt, SourceFile};
pub use provider::{AgentError, AgentProvider, Attempt, Model, Usage};
