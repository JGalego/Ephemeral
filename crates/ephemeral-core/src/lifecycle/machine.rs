//! The transition table and the persisted machine.
//!
//! [`LifecycleState::next`] is a **total function**: every (state, event) pair
//! yields either a new state or a typed error naming both. There is no catch-all
//! arm that quietly ignores an unexpected event, which is what stops an
//! autonomous loop from drifting into a combination nobody designed.
//!
//! [`Lifecycle`] wraps that function with everything a real application needs:
//! the repair budget, the state to resume to after an interruption, the history,
//! and the actor check.

use serde::{Deserialize, Serialize};

use super::{LifecycleEvent, LifecycleState, Transition, TransitionRequest};
use crate::{Timestamp, actor::Actor, now};

/// How many repair attempts an application gets before Ephemeral gives up.
///
/// Bounded by construction: an unbounded repair loop is a security and cost
/// problem, not merely an annoyance ([ADR-0008]).
///
/// [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md
pub const DEFAULT_REPAIR_BUDGET: u32 = 3;

/// Why a transition was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LifecycleError {
    /// The event has no meaning in this state.
    #[error("an application that is {from} cannot handle {event}")]
    IllegalTransition {
        /// The state the application was in.
        from: LifecycleState,
        /// The event that was raised.
        event: LifecycleEvent,
    },

    /// This actor is not permitted to raise this event.
    ///
    /// The security-relevant refusal: it is what stops the generation agent
    /// approving its own output or deleting an application.
    #[error("{actor} may not raise {event}; only {allowed} may")]
    UnauthorizedActor {
        /// The event that was attempted.
        event: LifecycleEvent,
        /// Who attempted it.
        actor: Actor,
        /// Who is permitted to, formatted for the message.
        allowed: String,
    },

    /// The repair budget is spent. Ephemeral stops rather than looping.
    #[error("the repair budget is spent after {attempts} of {budget} attempts")]
    RepairBudgetExhausted {
        /// How many repairs have been attempted.
        attempts: u32,
        /// How many were allowed.
        budget: u32,
    },

    /// An interruption is being resolved, but there is no recorded state to
    /// resume into.
    ///
    /// Refused rather than guessed: resuming into the wrong state could skip
    /// validation entirely.
    #[error("{event} cannot resume from {from}: no interrupted state was recorded")]
    NoResumeState {
        /// The state the application was in.
        from: LifecycleState,
        /// The event that was raised.
        event: LifecycleEvent,
    },

    /// The recorded resume state is not one Ephemeral could have been
    /// interrupted in.
    ///
    /// Only reachable through corrupted or tampered-with persisted state, which
    /// is exactly why it is checked.
    #[error("cannot resume into {state}: it is not an interruptible state")]
    NotResumable {
        /// The invalid resume state.
        state: LifecycleState,
    },
}

/// Where a transition leads.
///
/// Most transitions name a state outright. Resolving an interruption instead
/// returns to wherever the application was when it was interrupted, which is
/// known only to the machine — hence the second variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A fixed destination.
    State(LifecycleState),

    /// Back to the state that was interrupted.
    Resume,
}

/// The facts a transition needs beyond the state and the event.
///
/// Passed explicitly so that [`LifecycleState::next`] stays a pure, total
/// function of its inputs — the same context and the same event always produce
/// the same result, which is what makes the machine testable and replayable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionContext {
    /// The state to resume into after an interruption, if one was recorded.
    pub resume_state: Option<LifecycleState>,

    /// How many repairs have been attempted so far.
    pub repair_attempts: u32,

    /// How many repairs are allowed in total.
    pub repair_budget: u32,
}

impl Default for TransitionContext {
    fn default() -> Self {
        Self {
            resume_state: None,
            repair_attempts: 0,
            repair_budget: DEFAULT_REPAIR_BUDGET,
        }
    }
}

impl LifecycleState {
    /// Every event this state can handle, and where each one leads.
    ///
    /// This is the transition table. It is deliberately written out in full
    /// rather than derived, because it is the thing a reviewer needs to be able
    /// to read top to bottom and agree with.
    ///
    /// Some rules that are easy to miss, and are load-bearing:
    ///
    /// - **`Delete` is accepted from every state except `Deleted` itself.** A
    ///   user must always be able to stop an application, whatever it is doing.
    /// - **Interruptions are only offered from working states.** An application
    ///   that is `Ready` or `Running` is not waiting on Ephemeral, so it cannot
    ///   be interrupted into `PermissionRequired`.
    /// - **`Restore` leads out of `Deleted` into `Archived`, not into `Ready`.**
    ///   Recovering from the trash restores the record, not the ability to run;
    ///   the user restores again to make it runnable.
    /// - **Validation success never comes from `Repairing`.** A repair always
    ///   goes back through `Building` and `Validating`, so a fix is re-tested
    ///   rather than assumed.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "the transition table is the point; splitting it would hide it"
    )]
    pub fn outgoing(self) -> Vec<(LifecycleEvent, Target)> {
        use LifecycleEvent as E;
        use LifecycleState as S;
        use Target::{Resume, State};

        // Every state in which Ephemeral is mid-flight can be interrupted by a
        // permission request or a blocker, cancelled by the user, or deleted.
        let interruptible = |extra: Vec<(LifecycleEvent, Target)>| {
            let mut all = extra;
            all.extend([
                (E::PermissionRequested, State(S::PermissionRequired)),
                (E::Block, State(S::Blocked)),
                (E::Cancel, State(S::Cancelled)),
                (E::Delete, State(S::Deleted)),
            ]);
            all
        };

        match self {
            S::Requested => interruptible(vec![(E::Plan, State(S::Planning))]),
            S::Planning => interruptible(vec![(E::PlanCompleted, State(S::Generating))]),
            S::Generating => interruptible(vec![(E::GenerationCompleted, State(S::Building))]),
            S::Building => interruptible(vec![
                (E::BuildSucceeded, State(S::Validating)),
                (E::BuildFailed, State(S::BuildFailed)),
            ]),
            S::Validating => interruptible(vec![
                (E::ValidationPassed, State(S::Ready)),
                (E::ValidationFailed, State(S::ValidationFailed)),
            ]),
            S::Repairing => interruptible(vec![
                (E::RepairCompleted, State(S::Building)),
                (E::RepairFailed, State(S::ValidationFailed)),
            ]),

            // Interruptions. Resolving one returns to where work stopped; a
            // denial turns the interruption into a blocker the user must
            // resolve.
            S::PermissionRequired => vec![
                (E::PermissionGranted, Resume),
                (E::PermissionDenied, State(S::Blocked)),
                (E::Cancel, State(S::Cancelled)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Blocked => vec![
                (E::Unblock, Resume),
                (E::Retry, State(S::Planning)),
                (E::Cancel, State(S::Cancelled)),
                (E::Archive, State(S::Archived)),
                (E::Delete, State(S::Deleted)),
            ],

            // Built and available.
            S::Ready => vec![
                (E::Start, State(S::Starting)),
                (E::Archive, State(S::Archived)),
                (E::Expire, State(S::Archived)),
                (E::Delete, State(S::Deleted)),
            ],

            // Execution. Facts here are reported by the runtime.
            S::Starting => vec![
                (E::Started, State(S::Running)),
                (E::StartFailed, State(S::RuntimeFailed)),
                (E::RuntimeCrashed, State(S::RuntimeFailed)),
                (E::Stop, State(S::Stopping)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Running => vec![
                (E::Pause, State(S::Paused)),
                (E::Stop, State(S::Stopping)),
                (E::HealthDegraded, State(S::Unhealthy)),
                (E::RuntimeCrashed, State(S::RuntimeFailed)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Paused => vec![
                (E::Resume, State(S::Running)),
                (E::Stop, State(S::Stopping)),
                (E::RuntimeCrashed, State(S::RuntimeFailed)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Stopping => vec![
                (E::Stopped, State(S::Ready)),
                (E::RuntimeCrashed, State(S::RuntimeFailed)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Unhealthy => vec![
                (E::HealthRestored, State(S::Running)),
                (E::Stop, State(S::Stopping)),
                (E::RuntimeCrashed, State(S::RuntimeFailed)),
                (E::Delete, State(S::Deleted)),
            ],

            // Things went wrong. Repair is bounded; retry starts over; the user
            // can always put it away or throw it away.
            S::BuildFailed | S::ValidationFailed => vec![
                (E::Repair, State(S::Repairing)),
                (E::Retry, State(S::Planning)),
                (E::Archive, State(S::Archived)),
                (E::Delete, State(S::Deleted)),
            ],
            S::RuntimeFailed => vec![
                (E::Start, State(S::Starting)),
                (E::Repair, State(S::Repairing)),
                (E::Retry, State(S::Planning)),
                (E::Archive, State(S::Archived)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Cancelled => vec![
                (E::Retry, State(S::Planning)),
                (E::Archive, State(S::Archived)),
                (E::Delete, State(S::Deleted)),
            ],

            // Put away, and thrown away.
            S::Archived => vec![
                (E::Restore, State(S::Ready)),
                (E::Expire, State(S::Deleted)),
                (E::Delete, State(S::Deleted)),
            ],
            S::Deleted => vec![(E::Restore, State(S::Archived))],
        }
    }

    /// Applies an event to a state.
    ///
    /// Total and deterministic: the same state, event and context always give
    /// the same answer.
    ///
    /// # Errors
    ///
    /// - [`LifecycleError::IllegalTransition`] if the event has no meaning here.
    /// - [`LifecycleError::RepairBudgetExhausted`] if a repair was requested
    ///   after the budget was spent.
    /// - [`LifecycleError::NoResumeState`] or [`LifecycleError::NotResumable`]
    ///   if an interruption is being resolved but the recorded resume state is
    ///   missing or implausible.
    pub fn next(
        self,
        event: LifecycleEvent,
        context: &TransitionContext,
    ) -> Result<Self, LifecycleError> {
        let target = self
            .outgoing()
            .into_iter()
            .find_map(|(candidate, target)| (candidate == event).then_some(target))
            .ok_or(LifecycleError::IllegalTransition { from: self, event })?;

        if event == LifecycleEvent::Repair && context.repair_attempts >= context.repair_budget {
            return Err(LifecycleError::RepairBudgetExhausted {
                attempts: context.repair_attempts,
                budget: context.repair_budget,
            });
        }

        match target {
            Target::State(state) => Ok(state),
            Target::Resume => {
                let resume = context
                    .resume_state
                    .ok_or(LifecycleError::NoResumeState { from: self, event })?;
                if resume.is_resumable() {
                    Ok(resume)
                } else {
                    Err(LifecycleError::NotResumable { state: resume })
                }
            }
        }
    }
}

/// An application's lifecycle: where it is, how it got there, and what it is
/// still allowed to do.
///
/// Persisted with the application, so a crash mid-build resumes into a state
/// that is both valid and honest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    state: LifecycleState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_state: Option<LifecycleState>,

    repair_attempts: u32,
    repair_budget: u32,

    created_at: Timestamp,
    updated_at: Timestamp,

    #[serde(default)]
    history: Vec<Transition>,
}

impl Lifecycle {
    /// Starts a new lifecycle in [`LifecycleState::Requested`] with the default
    /// repair budget.
    #[must_use]
    pub fn new() -> Self {
        Self::with_repair_budget(DEFAULT_REPAIR_BUDGET)
    }

    /// Starts a new lifecycle with an explicit repair budget.
    #[must_use]
    pub fn with_repair_budget(repair_budget: u32) -> Self {
        let at = now();
        Self {
            state: LifecycleState::Requested,
            resume_state: None,
            repair_attempts: 0,
            repair_budget,
            created_at: at,
            updated_at: at,
            history: Vec::new(),
        }
    }

    /// Where the application is now.
    #[must_use]
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// Every transition, oldest first.
    #[must_use]
    pub fn history(&self) -> &[Transition] {
        &self.history
    }

    /// The most recent transition, if there has been one.
    #[must_use]
    pub fn last_transition(&self) -> Option<&Transition> {
        self.history.last()
    }

    /// When the lifecycle started.
    #[must_use]
    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// When the state last changed.
    #[must_use]
    pub fn updated_at(&self) -> Timestamp {
        self.updated_at
    }

    /// How many repairs have been attempted.
    #[must_use]
    pub fn repair_attempts(&self) -> u32 {
        self.repair_attempts
    }

    /// How many repairs are allowed in total.
    #[must_use]
    pub fn repair_budget(&self) -> u32 {
        self.repair_budget
    }

    /// The state work will resume into, if the application is interrupted.
    #[must_use]
    pub fn resume_state(&self) -> Option<LifecycleState> {
        self.resume_state
    }

    /// The context the transition table needs.
    fn context(&self) -> TransitionContext {
        TransitionContext {
            resume_state: self.resume_state,
            repair_attempts: self.repair_attempts,
            repair_budget: self.repair_budget,
        }
    }

    /// Applies a transition.
    ///
    /// The order of checks matters: the actor is verified *before* the
    /// transition table is consulted, so an unauthorised caller learns nothing
    /// about which transitions exist.
    ///
    /// Nothing is mutated unless every check passes — a refused transition
    /// leaves the lifecycle exactly as it was, including its history.
    ///
    /// # Errors
    ///
    /// [`LifecycleError::UnauthorizedActor`] if the actor may not raise this
    /// event, or anything [`LifecycleState::next`] returns.
    pub fn apply(&mut self, request: TransitionRequest) -> Result<&Transition, LifecycleError> {
        let event = request.event;
        let actor = request.actor;

        if !event.permits(actor) {
            let allowed = event
                .authorized_actors()
                .iter()
                .map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(LifecycleError::UnauthorizedActor {
                event,
                actor,
                allowed,
            });
        }

        let from = self.state;
        let to = from.next(event, &self.context())?;

        // Remember where to come back to when work is interrupted, and forget it
        // once the interruption is resolved. A resume state that is never
        // cleared would let a later, unrelated interruption resume into a stale
        // place.
        match event {
            LifecycleEvent::PermissionRequested | LifecycleEvent::Block => {
                if from.is_resumable() {
                    self.resume_state = Some(from);
                }
            }
            LifecycleEvent::PermissionGranted | LifecycleEvent::Unblock => {
                self.resume_state = None;
            }
            // Starting over discards the interrupted context along with the
            // repair budget: this is a fresh attempt, not a continuation.
            LifecycleEvent::Retry => {
                self.resume_state = None;
                self.repair_attempts = 0;
            }
            LifecycleEvent::Repair => {
                self.repair_attempts += 1;
            }
            _ => {}
        }

        self.state = to;
        self.updated_at = now();
        self.history.push(Transition::record(from, to, request));

        // Just pushed, so there is always a last element.
        Ok(self
            .history
            .last()
            .unwrap_or_else(|| unreachable!("a transition was just recorded")))
    }

    /// Whether this actor could raise this event right now.
    ///
    /// Used by interfaces to decide which buttons to offer, so that a user is
    /// never shown an action that would be refused.
    #[must_use]
    pub fn can_apply(&self, event: LifecycleEvent, actor: Actor) -> bool {
        event.permits(actor) && self.state.next(event, &self.context()).is_ok()
    }

    /// Every event this actor could raise right now.
    #[must_use]
    pub fn available_events(&self, actor: Actor) -> Vec<LifecycleEvent> {
        self.state
            .outgoing()
            .into_iter()
            .map(|(event, _)| event)
            .filter(|event| self.can_apply(*event, actor))
            .collect()
    }

    /// A plain-language account of what is happening and why.
    ///
    /// This is what a UI shows instead of a spinner: the state's own
    /// explanation, followed by the reason recorded for the most recent
    /// transition.
    #[must_use]
    pub fn explain(&self) -> String {
        let description = self.state.description();
        match self.last_transition() {
            Some(transition) if !transition.reason.is_empty() => {
                format!("{description} ({})", transition.reason)
            }
            _ => description.to_owned(),
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::TransitionError;

    fn ctx() -> TransitionContext {
        TransitionContext::default()
    }

    fn advance(lifecycle: &mut Lifecycle, event: LifecycleEvent, actor: Actor) {
        let from = lifecycle.state();
        if let Err(error) = lifecycle.apply(TransitionRequest::new(event, actor, "test")) {
            panic!("{event} by {actor} from {from} should be allowed: {error}");
        }
    }

    /// Drives an application from a fresh request to `Ready`, the way the
    /// orchestrator will.
    fn build_to_ready() -> Lifecycle {
        let mut lifecycle = Lifecycle::new();
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::GenerationCompleted,
            Actor::Agent,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::BuildSucceeded,
            Actor::Runtime,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::ValidationPassed,
            Actor::Runtime,
        );
        lifecycle
    }

    // --- the table itself ---------------------------------------------------

    #[test]
    fn the_happy_path_reaches_ready() {
        let lifecycle = build_to_ready();
        assert_eq!(lifecycle.state(), LifecycleState::Ready);
        assert_eq!(lifecycle.history().len(), 5);
        assert_eq!(lifecycle.history()[0].from, LifecycleState::Requested);
        assert_eq!(lifecycle.history()[4].to, LifecycleState::Ready);
    }

    #[test]
    fn the_full_journey_runs_stops_archives_restores_and_deletes() {
        let mut lifecycle = build_to_ready();

        advance(&mut lifecycle, LifecycleEvent::Start, Actor::User);
        advance(&mut lifecycle, LifecycleEvent::Started, Actor::Runtime);
        assert_eq!(lifecycle.state(), LifecycleState::Running);

        advance(&mut lifecycle, LifecycleEvent::Pause, Actor::User);
        advance(&mut lifecycle, LifecycleEvent::Resume, Actor::User);
        advance(&mut lifecycle, LifecycleEvent::Stop, Actor::User);
        advance(&mut lifecycle, LifecycleEvent::Stopped, Actor::Runtime);
        assert_eq!(lifecycle.state(), LifecycleState::Ready);

        advance(&mut lifecycle, LifecycleEvent::Archive, Actor::User);
        advance(&mut lifecycle, LifecycleEvent::Restore, Actor::User);
        assert_eq!(lifecycle.state(), LifecycleState::Ready);

        advance(&mut lifecycle, LifecycleEvent::Delete, Actor::User);
        assert_eq!(lifecycle.state(), LifecycleState::Deleted);
        assert!(!lifecycle.state().is_runnable());
    }

    /// The transition function must be total: no (state, event) pair may panic,
    /// and anything that is not in the table must be a named error.
    #[test]
    fn every_state_and_event_pair_is_defined() {
        for state in LifecycleState::ALL {
            for event in LifecycleEvent::ALL {
                let result = state.next(event, &ctx());
                if let Err(error) = result {
                    assert!(
                        matches!(
                            error,
                            LifecycleError::IllegalTransition { .. }
                                | LifecycleError::NoResumeState { .. }
                                | LifecycleError::RepairBudgetExhausted { .. }
                        ),
                        "{state} + {event} produced an unexpected error: {error}"
                    );
                }
            }
        }
    }

    #[test]
    fn illegal_transitions_name_both_sides() {
        let error = LifecycleState::Ready
            .next(LifecycleEvent::Started, &ctx())
            .unwrap_err();
        assert_eq!(
            error,
            LifecycleError::IllegalTransition {
                from: LifecycleState::Ready,
                event: LifecycleEvent::Started
            }
        );
        assert!(error.to_string().contains("ready"));
        assert!(error.to_string().contains("started"));
    }

    /// A user must always be able to get rid of an application, whatever it is
    /// doing.
    #[test]
    fn deletion_is_reachable_from_every_live_state() {
        for state in LifecycleState::ALL {
            if state == LifecycleState::Deleted {
                continue;
            }
            assert_eq!(
                state.next(LifecycleEvent::Delete, &ctx()),
                Ok(LifecycleState::Deleted),
                "a user must be able to delete an application that is {state}"
            );
        }
    }

    /// Only states Ephemeral could genuinely have been interrupted in offer an
    /// interruption, so a `Running` app cannot be diverted into a permission
    /// prompt and back.
    #[test]
    fn only_working_states_can_be_interrupted() {
        for state in LifecycleState::ALL {
            let interruptible = state
                .next(LifecycleEvent::PermissionRequested, &ctx())
                .is_ok();
            assert_eq!(
                interruptible,
                state.is_resumable(),
                "{state} disagrees about being interruptible"
            );
        }
    }

    #[test]
    fn a_repair_is_always_re_validated() {
        // The only way out of Repairing towards success is back through
        // Building, which forces Validating again.
        assert_eq!(
            LifecycleState::Repairing.next(LifecycleEvent::RepairCompleted, &ctx()),
            Ok(LifecycleState::Building)
        );
        assert!(
            LifecycleState::Repairing
                .next(LifecycleEvent::ValidationPassed, &ctx())
                .is_err(),
            "a repair must not be able to declare itself validated"
        );
    }

    // --- interruption and resumption ----------------------------------------

    #[test]
    fn a_permission_grant_resumes_where_work_stopped() {
        let mut lifecycle = Lifecycle::new();
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );

        advance(
            &mut lifecycle,
            LifecycleEvent::PermissionRequested,
            Actor::Ephemeral,
        );
        assert_eq!(lifecycle.state(), LifecycleState::PermissionRequired);
        assert_eq!(lifecycle.resume_state(), Some(LifecycleState::Generating));

        advance(
            &mut lifecycle,
            LifecycleEvent::PermissionGranted,
            Actor::User,
        );
        assert_eq!(lifecycle.state(), LifecycleState::Generating);
        assert_eq!(
            lifecycle.resume_state(),
            None,
            "the resume state must be consumed, not left to be reused later"
        );
    }

    #[test]
    fn a_denied_permission_blocks_and_can_still_be_unblocked() {
        let mut lifecycle = Lifecycle::new();
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PermissionRequested,
            Actor::Ephemeral,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::PermissionDenied,
            Actor::User,
        );

        assert_eq!(lifecycle.state(), LifecycleState::Blocked);
        assert_eq!(lifecycle.resume_state(), Some(LifecycleState::Planning));

        advance(&mut lifecycle, LifecycleEvent::Unblock, Actor::User);
        assert_eq!(lifecycle.state(), LifecycleState::Planning);
    }

    /// Corrupted persisted state must not be able to resume an application into
    /// a state it never legitimately reached — `Running`, for instance, would
    /// skip the runtime entirely.
    #[test]
    fn a_corrupted_resume_state_is_refused() {
        let context = TransitionContext {
            resume_state: Some(LifecycleState::Running),
            ..TransitionContext::default()
        };
        assert_eq!(
            LifecycleState::PermissionRequired.next(LifecycleEvent::PermissionGranted, &context),
            Err(LifecycleError::NotResumable {
                state: LifecycleState::Running
            })
        );
    }

    #[test]
    fn resuming_without_a_recorded_state_is_refused() {
        assert_eq!(
            LifecycleState::PermissionRequired.next(LifecycleEvent::PermissionGranted, &ctx()),
            Err(LifecycleError::NoResumeState {
                from: LifecycleState::PermissionRequired,
                event: LifecycleEvent::PermissionGranted
            })
        );
    }

    // --- the repair budget --------------------------------------------------

    #[test]
    fn the_repair_budget_is_enforced_and_then_stops_the_loop() {
        let mut lifecycle = Lifecycle::with_repair_budget(2);
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::GenerationCompleted,
            Actor::Agent,
        );

        for attempt in 1..=2 {
            advance(
                &mut lifecycle,
                LifecycleEvent::BuildSucceeded,
                Actor::Runtime,
            );
            advance(
                &mut lifecycle,
                LifecycleEvent::ValidationFailed,
                Actor::Runtime,
            );
            advance(&mut lifecycle, LifecycleEvent::Repair, Actor::Ephemeral);
            assert_eq!(lifecycle.repair_attempts(), attempt);
            advance(
                &mut lifecycle,
                LifecycleEvent::RepairCompleted,
                Actor::Agent,
            );
        }

        advance(
            &mut lifecycle,
            LifecycleEvent::BuildSucceeded,
            Actor::Runtime,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::ValidationFailed,
            Actor::Runtime,
        );

        let refused = lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::Repair,
                Actor::Ephemeral,
                "one more time",
            ))
            .unwrap_err();
        assert_eq!(
            refused,
            LifecycleError::RepairBudgetExhausted {
                attempts: 2,
                budget: 2
            }
        );
        assert_eq!(
            lifecycle.state(),
            LifecycleState::ValidationFailed,
            "a refused repair must leave the application where it was"
        );
    }

    #[test]
    fn retrying_resets_the_repair_budget() {
        let mut lifecycle = Lifecycle::with_repair_budget(1);
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::GenerationCompleted,
            Actor::Agent,
        );
        advance(&mut lifecycle, LifecycleEvent::BuildFailed, Actor::Runtime);
        advance(&mut lifecycle, LifecycleEvent::Repair, Actor::Ephemeral);
        assert_eq!(lifecycle.repair_attempts(), 1);

        advance(&mut lifecycle, LifecycleEvent::RepairFailed, Actor::Agent);
        advance(&mut lifecycle, LifecycleEvent::Retry, Actor::User);

        assert_eq!(lifecycle.state(), LifecycleState::Planning);
        assert_eq!(lifecycle.repair_attempts(), 0);
    }

    // --- actor authorisation ------------------------------------------------

    /// The central anti-injection property: whatever the model produces, the
    /// agent cannot approve its own work, authorise access, or destroy data.
    #[test]
    fn the_agent_cannot_approve_authorise_or_destroy() {
        let mut lifecycle = build_to_ready();
        let refused = lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::Delete,
                Actor::Agent,
                "the plan says to clean up",
            ))
            .unwrap_err();
        assert!(matches!(refused, LifecycleError::UnauthorizedActor { .. }));
        assert_eq!(lifecycle.state(), LifecycleState::Ready);

        let mut validating = Lifecycle::new();
        advance(&mut validating, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut validating,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );
        advance(
            &mut validating,
            LifecycleEvent::GenerationCompleted,
            Actor::Agent,
        );
        advance(
            &mut validating,
            LifecycleEvent::BuildSucceeded,
            Actor::Runtime,
        );

        let refused = validating
            .apply(TransitionRequest::new(
                LifecycleEvent::ValidationPassed,
                Actor::Agent,
                "I checked it myself",
            ))
            .unwrap_err();
        assert!(matches!(refused, LifecycleError::UnauthorizedActor { .. }));
        assert_eq!(validating.state(), LifecycleState::Validating);
    }

    /// The actor check runs before the transition table, so an unauthorised
    /// caller cannot probe which transitions exist.
    #[test]
    fn authorisation_is_checked_before_the_transition_table() {
        let mut lifecycle = Lifecycle::new();
        let error = lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::Delete,
                Actor::Agent,
                "probe",
            ))
            .unwrap_err();
        assert!(matches!(error, LifecycleError::UnauthorizedActor { .. }));

        // The same event from an illegal state is *also* an authorisation error
        // for this actor, revealing nothing about the table.
        let mut deleted = build_to_ready();
        advance(&mut deleted, LifecycleEvent::Delete, Actor::User);
        let error = deleted
            .apply(TransitionRequest::new(
                LifecycleEvent::Delete,
                Actor::Agent,
                "probe",
            ))
            .unwrap_err();
        assert!(matches!(error, LifecycleError::UnauthorizedActor { .. }));
    }

    #[test]
    fn a_refused_transition_changes_nothing() {
        let mut lifecycle = build_to_ready();
        let before = lifecycle.clone();

        assert!(
            lifecycle
                .apply(TransitionRequest::new(
                    LifecycleEvent::Started,
                    Actor::Runtime,
                    "nope"
                ))
                .is_err()
        );
        assert_eq!(lifecycle, before, "a refused transition must be a no-op");
    }

    // --- affordances and explanations ---------------------------------------

    #[test]
    fn available_events_reflect_both_the_state_and_the_actor() {
        let lifecycle = build_to_ready();

        let user = lifecycle.available_events(Actor::User);
        assert!(user.contains(&LifecycleEvent::Start));
        assert!(user.contains(&LifecycleEvent::Archive));
        assert!(user.contains(&LifecycleEvent::Delete));

        let agent = lifecycle.available_events(Actor::Agent);
        assert!(
            agent.is_empty(),
            "the agent should have nothing to do with a ready application, got {agent:?}"
        );

        let runtime = lifecycle.available_events(Actor::Runtime);
        assert!(runtime.is_empty());
    }

    #[test]
    fn can_apply_agrees_with_apply() {
        for event in LifecycleEvent::ALL {
            for actor in Actor::ALL {
                let mut lifecycle = build_to_ready();
                let predicted = lifecycle.can_apply(event, actor);
                let actual = lifecycle
                    .apply(TransitionRequest::new(event, actor, "check"))
                    .is_ok();
                assert_eq!(
                    predicted, actual,
                    "can_apply disagreed with apply for {event} by {actor}"
                );
            }
        }
    }

    #[test]
    fn the_machine_explains_itself() {
        let mut lifecycle = Lifecycle::new();
        lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "working out what this needs",
            ))
            .unwrap();

        let explanation = lifecycle.explain();
        assert!(explanation.contains("working out what this needs"));
        assert!(explanation.starts_with(LifecycleState::Planning.description()));
    }

    // --- persistence --------------------------------------------------------

    #[test]
    fn the_machine_round_trips_through_json() {
        let mut lifecycle = build_to_ready();
        lifecycle
            .apply(
                TransitionRequest::new(LifecycleEvent::Start, Actor::User, "run it")
                    .with_metadata("port", "8080"),
            )
            .unwrap();

        let json = serde_json::to_string(&lifecycle).unwrap();
        let restored: Lifecycle = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, lifecycle);
        assert_eq!(restored.state(), LifecycleState::Starting);
        assert_eq!(restored.history().len(), 6);
    }

    #[test]
    fn history_records_actor_reason_and_error() {
        let mut lifecycle = Lifecycle::new();
        advance(&mut lifecycle, LifecycleEvent::Plan, Actor::Ephemeral);
        advance(
            &mut lifecycle,
            LifecycleEvent::PlanCompleted,
            Actor::Ephemeral,
        );
        advance(
            &mut lifecycle,
            LifecycleEvent::GenerationCompleted,
            Actor::Agent,
        );
        lifecycle
            .apply(
                TransitionRequest::new(
                    LifecycleEvent::BuildFailed,
                    Actor::Runtime,
                    "the base image could not be pulled",
                )
                .with_error(TransitionError::new("docker.pull_failed", "no such host")),
            )
            .unwrap();

        let last = lifecycle.last_transition().unwrap();
        assert_eq!(last.from, LifecycleState::Building);
        assert_eq!(last.to, LifecycleState::BuildFailed);
        assert_eq!(last.actor, Actor::Runtime);
        assert!(last.is_failure());
        assert_eq!(last.error.as_ref().unwrap().code, "docker.pull_failed");
    }
}
