//! The application lifecycle state machine.
//!
//! Every application Ephemeral creates moves through this machine, and the
//! machine is the thing the interface renders. That is deliberate: a user should
//! be able to read *why* their app is doing something, not watch a spinner and
//! guess.
//!
//! ```text
//!         REQUESTED
//!             │ plan
//!             ▼
//!         PLANNING ──────────────┐
//!             │ plan ok          │
//!             ▼                  │  any working state may be
//!         GENERATING             │  interrupted by
//!             │ generated        │
//!             ▼                  ├──▶ PERMISSION_REQUIRED ──▶ (resume)
//!          BUILDING ◀────┐       │
//!             │ built    │       ├──▶ BLOCKED
//!             ▼          │       │
//!         VALIDATING     │       └──▶ CANCELLED
//!           │      │     │
//!      pass │      │ fail│ repair (bounded)
//!           │      ▼     │
//!           │  VALIDATION_FAILED ──▶ REPAIRING ──┘
//!           ▼
//!         READY ⇄ STARTING ⇄ RUNNING ⇄ PAUSED
//!           │                   │
//!           │                   └──▶ UNHEALTHY ──▶ RUNTIME_FAILED
//!           ▼
//!        ARCHIVED ──restore──▶ READY
//!           │
//!           ▼
//!        DELETED  ──restore──▶ ARCHIVED   (until purged)
//! ```
//!
//! ## What makes this trustworthy
//!
//! - **Total.** [`LifecycleState::next`] is defined for every (state, event)
//!   pair. Illegal combinations are typed errors, never silent no-ops.
//! - **Explicit.** The table is written out in [`LifecycleState::outgoing`] so
//!   it can be read and reviewed rather than inferred from call sites.
//! - **Authorised.** Every event names the actors that may raise it, and the
//!   check happens here rather than in a prompt. The generation agent cannot
//!   approve its own output, grant a permission, or delete anything.
//! - **Bounded.** The repair budget lives with the machine, so an autonomous
//!   loop terminates by construction.
//! - **Recorded.** Every applied transition appends a [`Transition`] carrying
//!   the previous state, the new state, the event, the actor, the reason, the
//!   time, metadata and structured error information.
//! - **Persisted.** [`Lifecycle`] serialises whole, so a crash mid-build resumes
//!   into a state that is both valid and honest.
//!
//! See [ADR-0004] for the alternatives that were rejected.
//!
//! [ADR-0004]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0004-explicit-lifecycle-state-machine.md
//!
//! # Example
//!
//! ```
//! use ephemeral_core::{Actor, Lifecycle, LifecycleEvent, LifecycleState};
//! use ephemeral_core::lifecycle::TransitionRequest;
//!
//! let mut lifecycle = Lifecycle::new();
//! assert_eq!(lifecycle.state(), LifecycleState::Requested);
//!
//! lifecycle.apply(TransitionRequest::new(
//!     LifecycleEvent::Plan,
//!     Actor::Ephemeral,
//!     "working out what kind of app this needs to be",
//! ))?;
//!
//! assert_eq!(lifecycle.state(), LifecycleState::Planning);
//! assert!(lifecycle.explain().contains("working out"));
//!
//! // The generation agent cannot delete an application, whatever it was told.
//! assert!(lifecycle
//!     .apply(TransitionRequest::new(LifecycleEvent::Delete, Actor::Agent, "cleanup"))
//!     .is_err());
//! # Ok::<(), ephemeral_core::lifecycle::LifecycleError>(())
//! ```

mod event;
mod machine;
mod state;
mod transition;

pub use event::LifecycleEvent;
pub use machine::{DEFAULT_REPAIR_BUDGET, Lifecycle, LifecycleError, Target, TransitionContext};
pub use state::{LifecycleState, StateKind};
pub use transition::{Transition, TransitionError, TransitionRequest};
