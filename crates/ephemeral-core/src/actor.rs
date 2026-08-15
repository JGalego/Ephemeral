//! Who caused something to happen.
//!
//! An [`Actor`] is recorded on every lifecycle transition, every permission
//! grant and every audit entry. It answers the question an incident review
//! always asks first: *did a human decide this, or did the system?*
//!
//! Actors are distinct from [principals](crate::identity::Principal). A
//! principal *holds* permissions; an actor *acts*. The generation agent is an
//! actor but holds no permissions, and a generated application is a principal
//! whose actions are attributed to [`Actor::Runtime`] or [`Actor::Ephemeral`]
//! depending on what performed them.
//!
//! ## Why this is a security type
//!
//! Ephemeral runs an autonomous loop that plans, writes and repairs code. If
//! that loop could authorise its own privileged operations, then anything that
//! can steer the loop — prompt injection through a filename, a CSV cell, a
//! fetched web page — could authorise them too.
//!
//! So authority is attached to the actor, checked in this crate, and never
//! delegated to a prompt. [`Actor::Agent`] cannot grant a permission and cannot
//! delete an application. That restriction is structural: it holds regardless of
//! what the model was persuaded to output.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Who performed an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// A human being, acting deliberately.
    ///
    /// The only actor that may grant or deny a permission, purge data, or
    /// otherwise make a decision the system cannot undo.
    User,

    /// Ephemeral's own orchestration, carrying out work the user asked for.
    Ephemeral,

    /// The generation agent: planning, writing code, diagnosing, repairing.
    ///
    /// Deliberately the least privileged actor. Its output is untrusted input to
    /// the system, not instruction to it ([ADR-0008]).
    ///
    /// [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md
    Agent,

    /// A container or process runtime reporting something that happened to it —
    /// an exit, a crash, a health-check result.
    Runtime,

    /// The operating system, a scheduler, or a retention sweep.
    ///
    /// Used for actions nobody initiated interactively, such as an app expiring
    /// under its retention policy.
    System,
}

impl Actor {
    /// Every actor, in order of decreasing authority.
    ///
    /// Useful for exhaustively testing authorisation rules; the ordering is
    /// documentation, not an authority lattice — authority is decided per
    /// operation, not by rank.
    pub const ALL: [Self; 5] = [
        Self::User,
        Self::Ephemeral,
        Self::Agent,
        Self::Runtime,
        Self::System,
    ];

    /// Whether this actor is a human decision.
    ///
    /// Operations that are irreversible, that grant authority, or that expose
    /// user data require this to be true.
    #[must_use]
    pub fn is_human(self) -> bool {
        matches!(self, Self::User)
    }

    /// A short, lowercase machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Ephemeral => "ephemeral",
            Self::Agent => "agent",
            Self::Runtime => "runtime",
            Self::System => "system",
        }
    }

    /// How this actor is described to a user.
    ///
    /// Audit entries and lifecycle history are shown to people who did not write
    /// this code, so "the generation agent" beats "agent".
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::User => "you",
            Self::Ephemeral => "Ephemeral",
            Self::Agent => "the generation agent",
            Self::Runtime => "the runtime",
            Self::System => "the system",
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_user_is_human() {
        assert!(Actor::User.is_human());
        for actor in Actor::ALL.iter().filter(|a| **a != Actor::User) {
            assert!(
                !actor.is_human(),
                "{actor} must not count as a human decision"
            );
        }
    }

    #[test]
    fn all_lists_every_variant() {
        // If a variant is added without extending ALL, the authorisation tests
        // that iterate over it would silently stop covering it.
        assert_eq!(Actor::ALL.len(), 5);
        let mut seen = Actor::ALL.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            Actor::ALL.len(),
            "ALL must not contain duplicates"
        );
    }

    #[test]
    fn actors_round_trip_through_json() {
        for actor in Actor::ALL {
            let json = serde_json::to_string(&actor).unwrap();
            assert_eq!(json, format!("\"{}\"", actor.as_str()));
            assert_eq!(serde_json::from_str::<Actor>(&json).unwrap(), actor);
        }
    }
}
