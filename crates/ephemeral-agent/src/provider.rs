//! The trait every model implementation satisfies.
//!
//! Deliberately small. A provider plans, generates, and repairs; it does not
//! decide, execute, or persist. Everything it returns is validated by the caller
//! before anything happens, and nothing it returns can reach the permission
//! ledger or the lifecycle machine without a person or the state machine
//! agreeing first ([ADR-0008]).
//!
//! Synchronous, because Ephemeral generates one application at a time for one
//! person. An async trait here would put an async runtime in the dependency
//! graph of every crate that touches generation, for no benefit at this scale —
//! the same reasoning as [ADR-0014].
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md
//! [ADR-0014]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0014-drive-docker-through-its-cli.md

use ephemeral_core::manifest::GenerationBudget;

use crate::plan::{GeneratedApp, Plan, PlanError, RepairAttempt, SourceFile};

/// One model a service says it has.
///
/// Two fields, because two is what a person choosing needs: the name to send,
/// and something readable to pick from. Services disagree about whether the
/// second exists — Anthropic has `display_name`, Groq has `name`, plain OpenAI
/// has neither — so it falls back to the id rather than being absent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    /// What to put in a request. The only part that has to be exact.
    pub id: String,

    /// What to show somebody. The id, when the service offers nothing better.
    pub name: String,

    /// The largest reply this model will accept a request for, when the service
    /// says.
    ///
    /// Worth carrying because it is the setting most likely to be wrong and the
    /// hardest to guess. A model with a 16k window refuses a request for 32k
    /// outright, with a message about a field nobody set — showing the number
    /// next to the name turns that into something a person can see coming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<u32>,
}

impl Model {
    /// A model known only by its id.
    #[must_use]
    pub fn named(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            ceiling: None,
        }
    }

    /// A model with a name of its own.
    #[must_use]
    pub fn called(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            ceiling: None,
        }
    }

    /// The same, knowing how large a reply it will accept.
    #[must_use]
    pub fn holding(mut self, ceiling: Option<u32>) -> Self {
        self.ceiling = ceiling;
        self
    }
}

/// What a model cost.
///
/// Tracked so the budget in every manifest is enforced rather than declared.
/// Costs are recorded per attempt and accumulated by the caller, so a run that
/// is cancelled halfway still has an accurate total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// Tokens sent.
    pub input_tokens: u64,

    /// Tokens received.
    pub output_tokens: u64,

    /// What it cost, in cents, when the provider charges.
    ///
    /// Zero for a local model, which is why the budget's spend ceiling is
    /// optional.
    pub cents: u32,
}

impl Usage {
    /// Adds another attempt's cost to this one.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            cents: self.cents.saturating_add(other.cents),
        }
    }

    /// Whether this has already exceeded a budget's spend ceiling.
    #[must_use]
    pub fn exceeds(self, budget: &GenerationBudget) -> bool {
        budget
            .max_spend_cents
            .is_some_and(|limit| self.cents > limit)
    }

    /// How this is shown to a person.
    #[must_use]
    pub fn describe(self) -> String {
        if self.cents == 0 {
            return format!(
                "{} tokens in, {} out, at no cost",
                self.input_tokens, self.output_tokens
            );
        }

        format!(
            "{} tokens in, {} out, ${:.2}",
            self.input_tokens,
            self.output_tokens,
            f64::from(self.cents) / 100.0
        )
    }
}

/// One thing a provider produced, and what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt<T> {
    /// What came back.
    pub result: T,

    /// What it cost.
    pub usage: Usage,
}

impl<T> Attempt<T> {
    /// Pairs a result with its cost.
    pub const fn new(result: T, usage: Usage) -> Self {
        Self { result, usage }
    }
}

/// Something went wrong asking a model for something.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// The provider is not usable — no credential, no network, not installed.
    #[error("{provider} is not available: {reason}")]
    Unavailable {
        /// Which provider.
        provider: String,
        /// Why not, and what would fix it.
        reason: String,
    },

    /// The model returned something this version cannot read.
    ///
    /// Carries what came back, because a malformed response is the most common
    /// thing a person debugging generation needs to see.
    #[error("{provider} returned something Ephemeral could not read: {reason}")]
    Unreadable {
        /// Which provider.
        provider: String,
        /// What was wrong with it.
        reason: String,
        /// What came back.
        raw: String,
    },

    /// The model returned something structurally valid that Ephemeral will not
    /// act on.
    #[error(transparent)]
    Refused(#[from] PlanError),

    /// The run hit a ceiling.
    ///
    /// Not a failure of the model: a bound doing its job. Reported separately
    /// so the interface can say "this reached its limit" rather than "this
    /// broke".
    #[error("{what} reached its limit ({limit})")]
    BudgetExhausted {
        /// Which ceiling.
        what: String,
        /// What the ceiling was.
        limit: String,
    },

    /// The user stopped it.
    #[error("cancelled")]
    Cancelled,

    /// The provider failed for its own reasons.
    #[error("{provider} failed: {reason}")]
    Failed {
        /// Which provider.
        provider: String,
        /// What it said.
        reason: String,
    },
}

/// Somewhere Ephemeral can ask for an application to be written.
///
/// Implementations must hold three properties, none of which is enforced by a
/// prompt:
///
/// - **Everything returned is a proposal.** No method here can grant a
///   permission, change a limit, or move an application through its lifecycle,
///   because none of them returns anything capable of expressing that.
/// - **Output is structured and validated.** Free-form text is never
///   interpreted as an instruction to the system. A response that does not
///   parse is [`AgentError::Unreadable`], not a best-effort guess.
/// - **Nothing here executes anything.** Generated code runs in the sandbox
///   ([ADR-0005]) and nowhere else, least of all in the process that generated
///   it.
///
/// [ADR-0005]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0005-docker-first-runtime-abstraction.md
pub trait AgentProvider {
    /// The provider's name, for the interface and the audit log.
    ///
    /// A name, never a credential. This string is written to the audit record.
    fn name(&self) -> &'static str;

    /// Whether this provider can be used right now, and why not if it cannot.
    ///
    /// Must not require the network: an unavailable provider is a diagnosis
    /// with a remedy, reported by `ephemeral doctor`, rather than something
    /// discovered at the moment somebody asks for an application.
    ///
    /// # Errors
    ///
    /// [`AgentError::Unavailable`], naming what is missing and what would fix
    /// it.
    fn availability(&self) -> Result<(), AgentError>;

    /// Proposes what to build, from what the user asked for.
    ///
    /// # Errors
    ///
    /// [`AgentError`] if the provider cannot be reached, returns something
    /// unreadable, or returns a plan Ephemeral will not act on.
    fn plan(&self, intent: &str) -> Result<Attempt<Plan>, AgentError>;

    /// Writes the application a plan describes.
    ///
    /// # Errors
    ///
    /// [`AgentError`] as above.
    fn generate(&self, plan: &Plan) -> Result<Attempt<GeneratedApp>, AgentError>;

    /// Proposes a fix for something that failed.
    ///
    /// `failure` is the build or test output verbatim. It is untrusted input —
    /// it may contain anything a dependency, a compiler, or another
    /// participant's data put there — and a provider must treat it as data to
    /// be diagnosed rather than instructions to follow.
    ///
    /// # Errors
    ///
    /// [`AgentError`] as above.
    fn repair(
        &self,
        app: &GeneratedApp,
        files: &[SourceFile],
        failure: &str,
    ) -> Result<Attempt<RepairAttempt>, AgentError>;

    /// What this service says it can be asked for.
    ///
    /// Unlike [`Self::availability`], this *does* reach the network — that is
    /// the point of it. It is the one call that answers, together, the two
    /// questions somebody has before spending anything: can I reach this
    /// service with the credential I have, and what may I name as a model.
    /// Answering them separately would mean two ways to be almost-configured.
    ///
    /// A service that will not list its models can still generate. A client
    /// showing this should say what failed rather than treating it as proof
    /// that nothing will work.
    ///
    /// # Errors
    ///
    /// [`AgentError`] as above — most usefully the service's own words when a
    /// credential is wrong, which is the failure this exists to surface early.
    fn models(&self) -> Result<Vec<Model>, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_accumulates_across_attempts() {
        let first = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cents: 3,
        };
        let second = Usage {
            input_tokens: 200,
            output_tokens: 80,
            cents: 5,
        };

        let total = first.plus(second);
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 130);
        assert_eq!(total.cents, 8);
    }

    /// A cost total that wrapped would report a runaway run as free.
    #[test]
    fn usage_saturates_rather_than_wrapping() {
        let huge = Usage {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cents: u32::MAX,
        };

        let total = huge.plus(huge);
        assert_eq!(total.cents, u32::MAX);
        assert_eq!(total.input_tokens, u64::MAX);
    }

    #[test]
    fn a_spend_ceiling_is_compared_against_the_running_total() {
        let budget = GenerationBudget::default();
        let ceiling = budget.max_spend_cents.unwrap();

        assert!(
            !Usage {
                cents: ceiling,
                ..Usage::default()
            }
            .exceeds(&budget)
        );

        assert!(
            Usage {
                cents: ceiling + 1,
                ..Usage::default()
            }
            .exceeds(&budget)
        );
    }

    /// A local model costs nothing, which is why the ceiling is optional and
    /// its absence must not be read as zero.
    #[test]
    fn an_absent_spend_ceiling_is_not_a_ceiling_of_zero() {
        let unlimited = GenerationBudget {
            max_spend_cents: None,
            ..GenerationBudget::default()
        };

        assert!(
            !Usage {
                cents: 100_000,
                ..Usage::default()
            }
            .exceeds(&unlimited)
        );
    }

    #[test]
    fn usage_describes_itself_for_a_person() {
        let free = Usage {
            input_tokens: 10,
            output_tokens: 20,
            cents: 0,
        };
        assert!(free.describe().contains("no cost"), "{}", free.describe());

        let paid = Usage { cents: 250, ..free };
        assert!(paid.describe().contains("$2.50"), "{}", paid.describe());
    }

    /// A ceiling doing its job is not a failure, and must not read like one.
    #[test]
    fn hitting_a_limit_reads_differently_from_breaking() {
        let exhausted = AgentError::BudgetExhausted {
            what: "repair attempts".to_owned(),
            limit: "3".to_owned(),
        };

        let message = exhausted.to_string();
        assert!(message.contains("reached its limit"), "{message}");
        assert!(!message.contains("failed"), "{message}");
    }
}
