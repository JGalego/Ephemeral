//! The states an application can be in.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What a state means for the person looking at it.
///
/// Every state maps to exactly one kind, and the kind is what a UI keys its
/// treatment off — an icon, a colour, whether to show a spinner, whether to
/// offer a button. Grouping this way keeps the interface honest: a user should
/// never have to learn twenty state names to know whether they need to do
/// something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    /// Ephemeral is working. It will move on by itself; nothing is required of
    /// the user except patience (and the option to cancel).
    Working,

    /// Ephemeral cannot continue until a person decides something.
    AwaitingUser,

    /// Built and available, but not running.
    Idle,

    /// Running right now.
    Active,

    /// Something went wrong, or the work was stopped. The user can retry,
    /// archive or discard.
    Attention,

    /// Put away. Takes no resources, and can be restored.
    Archived,

    /// Ended. The application holds nothing and has no permissions. Until it is
    /// purged it can still be recovered from the trash.
    Deleted,
}

/// Where an application is in its life.
///
/// This is a closed set, and transitions between its members are an explicit,
/// total function — see [`LifecycleState::next`]. There is no way to set a
/// state directly, which is what stops an autonomous repair loop from wandering
/// into a combination nobody designed ([ADR-0004]).
///
/// [ADR-0004]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0004-explicit-lifecycle-state-machine.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LifecycleState {
    /// The user has asked for something. Nothing has been done yet.
    Requested,

    /// Ephemeral is working out what kind of application is needed and what it
    /// will need access to.
    Planning,

    /// The generation agent is writing the application.
    Generating,

    /// The application is being built and its runtime provisioned.
    Building,

    /// The application is being tested against the plan.
    Validating,

    /// Validation or the build failed, and the agent is attempting a fix. Repair
    /// is bounded; see [`crate::lifecycle::Lifecycle::repair_budget`].
    Repairing,

    /// Work is paused because a permission decision is needed from the user.
    ///
    /// The state to resume to is remembered, so granting or denying continues
    /// deterministically rather than restarting.
    PermissionRequired,

    /// Work is paused for a reason the user must resolve — a missing runtime, a
    /// denied permission, no disk space.
    Blocked,

    /// Built, validated and available to run.
    Ready,

    /// The runtime is starting the application.
    Starting,

    /// The application is running.
    Running,

    /// The application is running but suspended. It keeps its resources and can
    /// be resumed instantly.
    Paused,

    /// The runtime is shutting the application down.
    Stopping,

    /// Running, but failing its health checks.
    Unhealthy,

    /// The application could not be built.
    BuildFailed,

    /// The application was built but does not do what was asked, and repair did
    /// not fix it.
    ValidationFailed,

    /// The application failed while running.
    RuntimeFailed,

    /// The user stopped the work before it finished.
    Cancelled,

    /// Put away. Its data is kept, it uses no runtime resources, and it can be
    /// restored.
    Archived,

    /// Deleted. Runtime resources are gone and all permissions are revoked, but
    /// the record survives for the recovery period so the user can change their
    /// mind. Purging removes it entirely, at which point there is no state left
    /// to be in ([ADR-0009]).
    ///
    /// [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md
    Deleted,
}

impl LifecycleState {
    /// Every state, in roughly the order an application meets them.
    ///
    /// Used by exhaustive transition tests and by `ephemeral states`.
    pub const ALL: [Self; 20] = [
        Self::Requested,
        Self::Planning,
        Self::Generating,
        Self::Building,
        Self::Validating,
        Self::Repairing,
        Self::PermissionRequired,
        Self::Blocked,
        Self::Ready,
        Self::Starting,
        Self::Running,
        Self::Paused,
        Self::Stopping,
        Self::Unhealthy,
        Self::BuildFailed,
        Self::ValidationFailed,
        Self::RuntimeFailed,
        Self::Cancelled,
        Self::Archived,
        Self::Deleted,
    ];

    /// What this state means for the user.
    #[must_use]
    pub fn kind(self) -> StateKind {
        match self {
            Self::Requested
            | Self::Planning
            | Self::Generating
            | Self::Building
            | Self::Validating
            | Self::Repairing
            | Self::Starting
            | Self::Stopping => StateKind::Working,
            Self::PermissionRequired | Self::Blocked => StateKind::AwaitingUser,
            Self::Ready | Self::Paused => StateKind::Idle,
            Self::Running => StateKind::Active,
            Self::Unhealthy
            | Self::BuildFailed
            | Self::ValidationFailed
            | Self::RuntimeFailed
            | Self::Cancelled => StateKind::Attention,
            Self::Archived => StateKind::Archived,
            Self::Deleted => StateKind::Deleted,
        }
    }

    /// The machine-readable name, matching the serialised form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Planning => "planning",
            Self::Generating => "generating",
            Self::Building => "building",
            Self::Validating => "validating",
            Self::Repairing => "repairing",
            Self::PermissionRequired => "permission_required",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopping => "stopping",
            Self::Unhealthy => "unhealthy",
            Self::BuildFailed => "build_failed",
            Self::ValidationFailed => "validation_failed",
            Self::RuntimeFailed => "runtime_failed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }

    /// A short label for a UI badge.
    #[must_use]
    pub fn headline(self) -> &'static str {
        match self {
            Self::Requested => "Requested",
            Self::Planning => "Planning",
            Self::Generating => "Writing the app",
            Self::Building => "Building",
            Self::Validating => "Testing",
            Self::Repairing => "Fixing",
            Self::PermissionRequired => "Needs your permission",
            Self::Blocked => "Blocked",
            Self::Ready => "Ready",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Stopping => "Stopping",
            Self::Unhealthy => "Not healthy",
            Self::BuildFailed => "Could not build",
            Self::ValidationFailed => "Does not work correctly",
            Self::RuntimeFailed => "Crashed",
            Self::Cancelled => "Cancelled",
            Self::Archived => "Archived",
            Self::Deleted => "Deleted",
        }
    }

    /// A plain-language explanation, aimed at someone who is not a developer.
    ///
    /// This is design principle 7 made concrete: the interface explains what the
    /// system is doing instead of showing an unexplained spinner. Callers pair
    /// this with the reason recorded on the latest transition to produce, for
    /// example: *"Building — Ephemeral is installing its runtime."*
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Requested => "Ephemeral has your request and is about to start.",
            Self::Planning => {
                "Ephemeral is working out what kind of app this needs to be and what it \
                 will need access to."
            }
            Self::Generating => "Ephemeral is writing the app.",
            Self::Building => "Ephemeral is building the app and setting up what it needs to run.",
            Self::Validating => "Ephemeral is testing the app to check it does what you asked.",
            Self::Repairing => {
                "The app did not work, so Ephemeral is fixing it and will try again."
            }
            Self::PermissionRequired => {
                "Ephemeral needs your decision before it can continue. Nothing happens \
                 until you choose."
            }
            Self::Blocked => {
                "Ephemeral cannot continue until something is resolved. The reason is \
                 shown with this app."
            }
            Self::Ready => "The app is built and ready. It is not running and uses nothing.",
            Self::Starting => "The app is starting up.",
            Self::Running => "The app is running now.",
            Self::Paused => "The app is suspended. It keeps its state and can resume instantly.",
            Self::Stopping => "The app is shutting down.",
            Self::Unhealthy => {
                "The app is running but is not responding correctly. Ephemeral is \
                 watching it."
            }
            Self::BuildFailed => {
                "Ephemeral could not build this app, and repairing it did not help."
            }
            Self::ValidationFailed => {
                "The app was built but does not do what you asked, and repairing it did \
                 not help."
            }
            Self::RuntimeFailed => "The app stopped unexpectedly while running.",
            Self::Cancelled => "You stopped this before it finished.",
            Self::Archived => {
                "The app is put away. It uses no resources and its data is kept. You can \
                 restore it whenever you like."
            }
            Self::Deleted => {
                "The app is deleted. It cannot run and has no permissions. Its data is \
                 kept briefly in case you change your mind."
            }
        }
    }

    /// Whether Ephemeral is actively doing work in this state.
    #[must_use]
    pub fn is_working(self) -> bool {
        self.kind() == StateKind::Working
    }

    /// Whether the application can execute from this state.
    ///
    /// Note that [`LifecycleState::Deleted`] is *not* runnable: deletion revokes
    /// runtime access immediately, even though the record survives for the
    /// recovery period.
    #[must_use]
    pub fn is_runnable(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Whether the application currently holds runtime resources — a container,
    /// a process, a port.
    #[must_use]
    pub fn holds_runtime_resources(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Paused | Self::Stopping | Self::Unhealthy
        )
    }

    /// Whether the application's life has ended.
    ///
    /// An ended application holds no runtime resources and has no permissions.
    /// It is not, however, unreachable: deletion is recoverable until the
    /// application is purged, at which point the record is removed and there is
    /// no state left to be in ([ADR-0009]).
    ///
    /// [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md
    #[must_use]
    pub fn is_ended(self) -> bool {
        self.kind() == StateKind::Deleted
    }

    /// Whether an application in this state must already know what it runs on.
    ///
    /// Planning is what decides the runtime, so an application that has only
    /// been requested genuinely does not have one yet. From the first build
    /// onwards it must, because everything after that point acts on it.
    #[must_use]
    pub fn requires_runtime(self) -> bool {
        matches!(
            self,
            Self::Building
                | Self::Validating
                | Self::Repairing
                | Self::Ready
                | Self::Starting
                | Self::Running
                | Self::Paused
                | Self::Stopping
                | Self::Unhealthy
        )
    }

    /// Whether an interrupted transition can resume back into this state.
    ///
    /// Only states in which Ephemeral was mid-flight qualify. Without this, a
    /// crafted resume value could return an application to, say,
    /// [`LifecycleState::Running`] without ever passing through the runtime.
    #[must_use]
    pub fn is_resumable(self) -> bool {
        matches!(
            self,
            Self::Requested
                | Self::Planning
                | Self::Generating
                | Self::Building
                | Self::Validating
                | Self::Repairing
        )
    }
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_contains_every_state_exactly_once() {
        let mut seen = LifecycleState::ALL.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            LifecycleState::ALL.len(),
            "ALL must not contain duplicates"
        );
    }

    #[test]
    fn every_state_has_user_facing_text() {
        for state in LifecycleState::ALL {
            assert!(!state.headline().is_empty(), "{state} has no headline");
            assert!(
                state.description().len() > 20,
                "{state} needs a real explanation, not a label"
            );
            assert!(!state.as_str().is_empty());
        }
    }

    #[test]
    fn states_round_trip_through_json() {
        for state in LifecycleState::ALL {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            assert_eq!(
                serde_json::from_str::<LifecycleState>(&json).unwrap(),
                state
            );
        }
    }

    #[test]
    fn only_ready_is_runnable() {
        for state in LifecycleState::ALL {
            assert_eq!(
                state.is_runnable(),
                state == LifecycleState::Ready,
                "{state} disagrees about runnability"
            );
        }
    }

    /// A deleted application must not be able to run, and must not be reported
    /// as holding runtime resources. Deletion revokes capability immediately.
    #[test]
    fn deleted_holds_nothing_and_runs_nothing() {
        let deleted = LifecycleState::Deleted;
        assert!(!deleted.is_runnable());
        assert!(!deleted.holds_runtime_resources());
        assert!(deleted.is_ended());
    }

    #[test]
    fn archived_holds_no_runtime_resources() {
        assert!(!LifecycleState::Archived.holds_runtime_resources());
        assert!(!LifecycleState::Ready.holds_runtime_resources());
        assert!(LifecycleState::Running.holds_runtime_resources());
    }

    /// Only states Ephemeral could genuinely have been interrupted in may be
    /// resumed into, so an interruption cannot be used to reach a running state
    /// without the runtime.
    #[test]
    fn resumable_states_are_all_working_states() {
        for state in LifecycleState::ALL {
            if state.is_resumable() {
                assert!(
                    state.is_working(),
                    "{state} is resumable but is not a working state"
                );
            }
        }
        assert!(!LifecycleState::Running.is_resumable());
        assert!(!LifecycleState::Ready.is_resumable());
        assert!(!LifecycleState::Deleted.is_resumable());
    }

    /// Nothing before the first build needs a runtime, and everything that can
    /// execute does.
    #[test]
    fn a_runtime_is_required_from_the_first_build_onwards() {
        for state in [
            LifecycleState::Requested,
            LifecycleState::Planning,
            LifecycleState::Generating,
            LifecycleState::Cancelled,
            LifecycleState::Blocked,
            LifecycleState::PermissionRequired,
            LifecycleState::Deleted,
        ] {
            assert!(
                !state.requires_runtime(),
                "{state} should not need a runtime"
            );
        }
        for state in [
            LifecycleState::Building,
            LifecycleState::Ready,
            LifecycleState::Running,
        ] {
            assert!(state.requires_runtime(), "{state} must have a runtime");
        }

        for state in LifecycleState::ALL {
            if state.is_runnable() || state.holds_runtime_resources() {
                assert!(
                    state.requires_runtime(),
                    "{state} can execute, so it must know what it runs on"
                );
            }
        }
    }

    #[test]
    fn only_deleted_counts_as_ended() {
        let ended: Vec<_> = LifecycleState::ALL
            .into_iter()
            .filter(|s| s.is_ended())
            .collect();
        assert_eq!(ended, vec![LifecycleState::Deleted]);
    }
}
