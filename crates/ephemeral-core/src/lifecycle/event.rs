//! The events that drive lifecycle transitions, and who is allowed to raise
//! them.
//!
//! States never change by assignment. Something *happens*, and the state machine
//! decides what that means. Modelling it this way is what makes the transition
//! table reviewable: you can read off exactly which occurrences can move an
//! application forward, and exactly who is trusted to report them.
//!
//! ## Actor authorisation is a security control
//!
//! Each event names the actors permitted to raise it, and
//! [`Lifecycle::apply`](crate::lifecycle::Lifecycle::apply) refuses the rest.
//! Two restrictions carry most of the weight:
//!
//! - [`Actor::Agent`] cannot raise [`LifecycleEvent::ValidationPassed`]. The
//!   thing that wrote the code cannot be the thing that declares it correct.
//! - [`Actor::Agent`] cannot raise [`LifecycleEvent::Delete`],
//!   [`LifecycleEvent::PermissionGranted`] or [`LifecycleEvent::Cancel`]. An
//!   agent steered by prompt injection still cannot destroy a user's data or
//!   authorise anything.
//!
//! These hold no matter what a model was persuaded to output, because they are
//! checked here rather than requested in a prompt.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::actor::Actor;

/// Something that happened to an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LifecycleEvent {
    /// Ephemeral picked the request up and started planning.
    Plan,

    /// Planning produced a plan.
    PlanCompleted,

    /// The agent finished writing the application.
    GenerationCompleted,

    /// The build succeeded.
    BuildSucceeded,

    /// The build failed.
    BuildFailed,

    /// Validation passed: the application does what was asked.
    ValidationPassed,

    /// Validation failed: the application does not do what was asked.
    ValidationFailed,

    /// A repair attempt is starting. Refused once the repair budget is spent.
    Repair,

    /// The agent produced a fix; the application will be rebuilt.
    RepairCompleted,

    /// The repair attempt itself failed.
    RepairFailed,

    /// A permission decision is needed before work can continue.
    PermissionRequested,

    /// The user granted the permission. Work resumes where it left off.
    PermissionGranted,

    /// The user denied the permission. Work cannot continue.
    PermissionDenied,

    /// Something outside Ephemeral's control is preventing progress.
    Block,

    /// The blocking condition was resolved. Work resumes where it left off.
    Unblock,

    /// The user stopped the work before it finished.
    Cancel,

    /// Try again after a failure or a cancellation.
    Retry,

    /// Start the application.
    Start,

    /// The runtime reports the application is up.
    Started,

    /// The runtime could not start the application.
    StartFailed,

    /// Suspend the application without shutting it down.
    Pause,

    /// Resume a suspended application.
    Resume,

    /// Shut the application down.
    Stop,

    /// The runtime reports the application is down.
    Stopped,

    /// Health checks are failing.
    HealthDegraded,

    /// Health checks are passing again.
    HealthRestored,

    /// The application died unexpectedly.
    RuntimeCrashed,

    /// Put the application away, keeping its data.
    Archive,

    /// Bring an archived application back.
    Restore,

    /// Delete the application: destroy its runtime resources and revoke every
    /// permission. Its data survives the recovery period.
    Delete,

    /// The retention policy elapsed.
    Expire,
}

impl LifecycleEvent {
    /// Every event. Used by the exhaustive transition tests.
    pub const ALL: [Self; 31] = [
        Self::Plan,
        Self::PlanCompleted,
        Self::GenerationCompleted,
        Self::BuildSucceeded,
        Self::BuildFailed,
        Self::ValidationPassed,
        Self::ValidationFailed,
        Self::Repair,
        Self::RepairCompleted,
        Self::RepairFailed,
        Self::PermissionRequested,
        Self::PermissionGranted,
        Self::PermissionDenied,
        Self::Block,
        Self::Unblock,
        Self::Cancel,
        Self::Retry,
        Self::Start,
        Self::Started,
        Self::StartFailed,
        Self::Pause,
        Self::Resume,
        Self::Stop,
        Self::Stopped,
        Self::HealthDegraded,
        Self::HealthRestored,
        Self::RuntimeCrashed,
        Self::Archive,
        Self::Restore,
        Self::Delete,
        Self::Expire,
    ];

    /// The machine-readable name, matching the serialised form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::PlanCompleted => "plan_completed",
            Self::GenerationCompleted => "generation_completed",
            Self::BuildSucceeded => "build_succeeded",
            Self::BuildFailed => "build_failed",
            Self::ValidationPassed => "validation_passed",
            Self::ValidationFailed => "validation_failed",
            Self::Repair => "repair",
            Self::RepairCompleted => "repair_completed",
            Self::RepairFailed => "repair_failed",
            Self::PermissionRequested => "permission_requested",
            Self::PermissionGranted => "permission_granted",
            Self::PermissionDenied => "permission_denied",
            Self::Block => "block",
            Self::Unblock => "unblock",
            Self::Cancel => "cancel",
            Self::Retry => "retry",
            Self::Start => "start",
            Self::Started => "started",
            Self::StartFailed => "start_failed",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
            Self::Stopped => "stopped",
            Self::HealthDegraded => "health_degraded",
            Self::HealthRestored => "health_restored",
            Self::RuntimeCrashed => "runtime_crashed",
            Self::Archive => "archive",
            Self::Restore => "restore",
            Self::Delete => "delete",
            Self::Expire => "expire",
        }
    }

    /// Which actors may raise this event.
    ///
    /// Anything not listed is refused by
    /// [`Lifecycle::apply`](crate::lifecycle::Lifecycle::apply) with
    /// [`LifecycleError::UnauthorizedActor`](crate::lifecycle::LifecycleError::UnauthorizedActor).
    ///
    /// The rules, and why:
    ///
    /// - **Decisions belong to the user.** Granting, denying, cancelling,
    ///   restoring and deleting are `User`-only. No autonomous component may
    ///   make a choice the user would expect to make themselves.
    /// - **The agent never signs off its own work.** `ValidationPassed` excludes
    ///   `Agent`, so a generated application cannot be declared correct by the
    ///   thing that generated it.
    /// - **Facts about execution come from the runtime.** `Started`, `Stopped`,
    ///   `RuntimeCrashed` and the health events are reported by whatever is
    ///   actually running the code, not asserted by the orchestrator.
    /// - **Expiry is the system's.** Retention sweeps are not user actions and
    ///   are attributed honestly.
    #[must_use]
    #[allow(
        clippy::match_same_arms,
        reason = "arms are grouped by why the rule exists, not by what it evaluates to; \
                  merging them would hide the reasoning this table is reviewed for"
    )]
    pub fn authorized_actors(self) -> &'static [Actor] {
        use Actor::{Agent, Ephemeral, Runtime, System, User};

        match self {
            // Orchestration: Ephemeral drives, the agent reports its own work.
            Self::Plan | Self::PlanCompleted => &[Ephemeral],
            Self::GenerationCompleted | Self::RepairCompleted | Self::RepairFailed => {
                &[Agent, Ephemeral]
            }
            Self::Repair => &[Ephemeral],

            // Build and validation results are facts, reported by whatever ran
            // them. The agent is excluded from all of these.
            Self::BuildSucceeded | Self::BuildFailed => &[Ephemeral, Runtime],
            Self::ValidationPassed | Self::ValidationFailed => &[Ephemeral, Runtime],

            // Permission decisions are the user's alone.
            Self::PermissionGranted | Self::PermissionDenied => &[User],
            Self::PermissionRequested => &[Ephemeral, Agent],

            // Blocking is observed; unblocking may be the user resolving it or
            // Ephemeral noticing it resolved itself.
            Self::Block => &[Ephemeral, Runtime, System],
            Self::Unblock => &[User, Ephemeral],

            // Stopping the work, and trying again.
            Self::Cancel => &[User],
            Self::Retry => &[User, Ephemeral],

            // Execution control: the user asks, Ephemeral may also act on a
            // schedule or a policy.
            Self::Start | Self::Stop | Self::Pause | Self::Resume => &[User, Ephemeral],

            // Execution facts: only the thing doing the running may report these.
            Self::Started
            | Self::StartFailed
            | Self::Stopped
            | Self::HealthDegraded
            | Self::HealthRestored
            | Self::RuntimeCrashed => &[Runtime],

            // Lifecycle management. Archiving may also be a retention sweep.
            Self::Archive => &[User, System],
            Self::Restore | Self::Delete => &[User],
            Self::Expire => &[System],
        }
    }

    /// Whether `actor` may raise this event.
    #[must_use]
    pub fn permits(self, actor: Actor) -> bool {
        self.authorized_actors().contains(&actor)
    }

    /// A plain-language description of the event, for history and audit views.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Plan => "started planning",
            Self::PlanCompleted => "finished planning",
            Self::GenerationCompleted => "finished writing the app",
            Self::BuildSucceeded => "built the app",
            Self::BuildFailed => "could not build the app",
            Self::ValidationPassed => "tested the app and it worked",
            Self::ValidationFailed => "tested the app and it did not work",
            Self::Repair => "started fixing the app",
            Self::RepairCompleted => "finished a fix",
            Self::RepairFailed => "could not fix the app",
            Self::PermissionRequested => "asked for a permission",
            Self::PermissionGranted => "granted a permission",
            Self::PermissionDenied => "denied a permission",
            Self::Block => "hit something it could not resolve",
            Self::Unblock => "resolved what was blocking it",
            Self::Cancel => "cancelled the work",
            Self::Retry => "tried again",
            Self::Start => "asked the app to start",
            Self::Started => "started the app",
            Self::StartFailed => "could not start the app",
            Self::Pause => "paused the app",
            Self::Resume => "resumed the app",
            Self::Stop => "asked the app to stop",
            Self::Stopped => "stopped the app",
            Self::HealthDegraded => "noticed the app is unhealthy",
            Self::HealthRestored => "noticed the app recovered",
            Self::RuntimeCrashed => "reported the app crashed",
            Self::Archive => "archived the app",
            Self::Restore => "restored the app",
            Self::Delete => "deleted the app",
            Self::Expire => "expired the app under its retention policy",
        }
    }
}

impl fmt::Display for LifecycleEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_contains_every_event_exactly_once() {
        let mut seen = LifecycleEvent::ALL.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), LifecycleEvent::ALL.len());
    }

    #[test]
    fn every_event_has_at_least_one_authorized_actor() {
        for event in LifecycleEvent::ALL {
            assert!(
                !event.authorized_actors().is_empty(),
                "{event} can never be raised by anyone"
            );
            assert!(!event.describe().is_empty());
        }
    }

    /// The generation agent must not be able to declare its own output correct,
    /// authorise anything, or destroy anything. This is the structural defence
    /// against a steered or injected agent.
    #[test]
    fn the_agent_cannot_approve_authorise_or_destroy() {
        for event in [
            LifecycleEvent::ValidationPassed,
            LifecycleEvent::PermissionGranted,
            LifecycleEvent::PermissionDenied,
            LifecycleEvent::Delete,
            LifecycleEvent::Cancel,
            LifecycleEvent::Archive,
            LifecycleEvent::Restore,
            LifecycleEvent::Start,
            LifecycleEvent::Expire,
        ] {
            assert!(
                !event.permits(Actor::Agent),
                "the agent must not be able to raise {event}"
            );
        }
    }

    /// Decisions a person would expect to make themselves stay with the person.
    #[test]
    fn only_the_user_decides_permissions_and_destruction() {
        for event in [
            LifecycleEvent::PermissionGranted,
            LifecycleEvent::PermissionDenied,
            LifecycleEvent::Delete,
            LifecycleEvent::Restore,
            LifecycleEvent::Cancel,
        ] {
            assert_eq!(
                event.authorized_actors(),
                &[Actor::User],
                "{event} must be a user-only decision"
            );
        }
    }

    /// Facts about execution are reported by the thing executing, so the
    /// orchestrator cannot assert that something ran when it did not.
    #[test]
    fn execution_facts_come_only_from_the_runtime() {
        for event in [
            LifecycleEvent::Started,
            LifecycleEvent::StartFailed,
            LifecycleEvent::Stopped,
            LifecycleEvent::HealthDegraded,
            LifecycleEvent::HealthRestored,
            LifecycleEvent::RuntimeCrashed,
        ] {
            assert_eq!(
                event.authorized_actors(),
                &[Actor::Runtime],
                "{event} must be reported by the runtime alone"
            );
        }
    }

    #[test]
    fn events_round_trip_through_json() {
        for event in LifecycleEvent::ALL {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(json, format!("\"{}\"", event.as_str()));
            assert_eq!(
                serde_json::from_str::<LifecycleEvent>(&json).unwrap(),
                event
            );
        }
    }
}
