//! The record kept for every lifecycle transition.
//!
//! A transition is not a log line. It is a structured, persisted fact that the
//! interface renders, the audit trail references, and an incident review reads.
//! Every one of them answers the same questions: what changed, because of what,
//! decided by whom, when, and — when something went wrong — what exactly.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{LifecycleEvent, LifecycleState};
use crate::{Timestamp, actor::Actor, now};

/// Structured error information attached to a transition.
///
/// Kept separate from the human-readable `reason` so that failures can be
/// matched on, aggregated and compared across runs without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionError {
    /// A stable, machine-readable code, such as `docker.unavailable` or
    /// `test.failed`.
    pub code: String,

    /// A one-line description suitable for showing to a user.
    pub message: String,

    /// Detail for a developer: a stack trace, compiler output, a test report.
    ///
    /// Never contains secrets: redaction runs on the write path, before this
    /// reaches storage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl TransitionError {
    /// Creates an error record.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    /// Attaches developer-facing detail.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A request to move an application to a new state.
///
/// Built by whoever observed the event, then handed to
/// [`Lifecycle::apply`](super::Lifecycle::apply), which decides whether it is
/// allowed and records the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRequest {
    /// What happened.
    pub event: LifecycleEvent,

    /// Who says so.
    pub actor: Actor,

    /// Why, in language a user can read.
    ///
    /// This is the text that turns *"Building"* into *"Building — Ephemeral is
    /// installing its runtime"*, so it is worth writing properly.
    pub reason: String,

    /// Anything else worth keeping: a build number, an image digest, an exit
    /// code, a duration.
    pub metadata: BTreeMap<String, String>,

    /// Structured error information, when the event represents a failure.
    pub error: Option<TransitionError>,
}

impl TransitionRequest {
    /// Creates a request.
    pub fn new(event: LifecycleEvent, actor: Actor, reason: impl Into<String>) -> Self {
        Self {
            event,
            actor,
            reason: reason.into(),
            metadata: BTreeMap::new(),
            error: None,
        }
    }

    /// Adds a metadata entry.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Attaches structured error information.
    #[must_use]
    pub fn with_error(mut self, error: TransitionError) -> Self {
        self.error = Some(error);
        self
    }
}

/// A transition that actually happened.
///
/// Appended to the application's history and never modified afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// The state before.
    pub from: LifecycleState,

    /// The state after.
    pub to: LifecycleState,

    /// What caused the change.
    pub event: LifecycleEvent,

    /// Who caused it.
    pub actor: Actor,

    /// Why, in language a user can read.
    pub reason: String,

    /// When, in UTC.
    pub at: Timestamp,

    /// Additional context.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,

    /// Structured error information, when this transition represents a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TransitionError>,
}

impl Transition {
    /// Records a transition that has been authorised and applied.
    ///
    /// Only [`Lifecycle::apply`](super::Lifecycle::apply) calls this — a
    /// transition record must never exist for a change that did not pass the
    /// transition table and the actor check.
    pub(super) fn record(
        from: LifecycleState,
        to: LifecycleState,
        request: TransitionRequest,
    ) -> Self {
        Self {
            from,
            to,
            event: request.event,
            actor: request.actor,
            reason: request.reason,
            at: now(),
            metadata: request.metadata,
            error: request.error,
        }
    }

    /// A one-line, plain-language account of this transition.
    ///
    /// For example: *"Ephemeral built the app — the runtime image was already
    /// available."*
    #[must_use]
    pub fn explain(&self) -> String {
        let who = self.actor.describe();
        let what = self.event.describe();

        // Capitalise the sentence without assuming ASCII.
        let mut subject = who.chars();
        let subject = match subject.next() {
            Some(first) => first.to_uppercase().collect::<String>() + subject.as_str(),
            None => String::new(),
        };

        if self.reason.is_empty() {
            format!("{subject} {what}.")
        } else {
            format!("{subject} {what} — {}", self.reason)
        }
    }

    /// Whether this transition represents something going wrong.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Transition {
        Transition::record(
            LifecycleState::Generating,
            LifecycleState::Building,
            TransitionRequest::new(
                LifecycleEvent::GenerationCompleted,
                Actor::Agent,
                "the app is a small Python script with no dependencies",
            )
            .with_metadata("files", "3"),
        )
    }

    #[test]
    fn transitions_explain_themselves_in_plain_language() {
        let explanation = sample().explain();
        assert_eq!(
            explanation,
            "The generation agent finished writing the app — the app is a small Python \
             script with no dependencies"
        );
    }

    #[test]
    fn explanation_survives_an_empty_reason() {
        let transition = Transition::record(
            LifecycleState::Ready,
            LifecycleState::Starting,
            TransitionRequest::new(LifecycleEvent::Start, Actor::User, ""),
        );
        assert_eq!(transition.explain(), "You asked the app to start.");
    }

    #[test]
    fn transitions_round_trip_through_json() {
        let transition = sample();
        let json = serde_json::to_string(&transition).unwrap();
        let parsed: Transition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, transition);
    }

    #[test]
    fn empty_metadata_and_absent_errors_are_omitted() {
        let transition = Transition::record(
            LifecycleState::Requested,
            LifecycleState::Planning,
            TransitionRequest::new(LifecycleEvent::Plan, Actor::Ephemeral, "picked up"),
        );
        let json = serde_json::to_string(&transition).unwrap();
        assert!(!json.contains("metadata"), "{json}");
        assert!(!json.contains("error"), "{json}");
    }

    #[test]
    fn failures_carry_structured_error_information() {
        let transition = Transition::record(
            LifecycleState::Building,
            LifecycleState::BuildFailed,
            TransitionRequest::new(
                LifecycleEvent::BuildFailed,
                Actor::Runtime,
                "the base image could not be pulled",
            )
            .with_error(
                TransitionError::new("docker.pull_failed", "could not pull python:3.12-slim")
                    .with_detail("dial tcp: lookup registry-1.docker.io: no such host"),
            ),
        );

        assert!(transition.is_failure());
        let error = transition.error.as_ref().unwrap();
        assert_eq!(error.code, "docker.pull_failed");
        assert!(error.detail.is_some());
        assert!(!sample().is_failure());
    }
}
