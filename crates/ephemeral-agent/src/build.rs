//! The bounded build-and-repair loop.
//!
//! Plan, generate, build, test, and — when something fails — diagnose and try
//! again, up to a ceiling. This is the most intricate control flow in the
//! product and the thing most likely to run away, so every axis it can consume
//! is bounded and every bound is a test.
//!
//! ## What bounds it
//!
//! | Bound | Comes from | What it stops |
//! |---|---|---|
//! | Repair attempts | [`GenerationBudget::max_repairs`] | An agent looping on a problem it cannot solve |
//! | Wall clock | [`GenerationBudget::max_duration`] | A run that hangs rather than fails |
//! | Spend | [`GenerationBudget::max_spend_cents`] | A surprise on a card |
//! | Cancellation | The user | Everything, at any point |
//!
//! Hitting a bound is [`AgentError::BudgetExhausted`], which is deliberately
//! not the same thing as a failure: a ceiling doing its job should not read like
//! something broke.
//!
//! ## What this does not do
//!
//! It does not touch the filesystem, start a container, or apply a lifecycle
//! transition. It takes a [`Builder`] — whatever knows how to turn source into
//! a working application — and reports what happened. The caller owns the
//! consequences, which is what keeps the untrusted half of the system on the
//! far side of a trait.

use ephemeral_core::{Recipe, manifest::GenerationBudget};

use crate::{
    plan::{GeneratedApp, SourceFile},
    provider::{AgentError, AgentProvider, Usage},
};

/// Whatever can turn generated source into something that runs.
///
/// A trait rather than the runtime directly, so the loop can be tested without
/// a container daemon and so nothing about generation depends on Docker.
pub trait Builder {
    /// Builds and tests an application.
    ///
    /// `Ok` means it built and its tests passed. `Err` carries the output
    /// verbatim, because that output is what a repair attempt reads — and
    /// summarising it here would throw away the only thing that can fix the
    /// problem.
    ///
    /// # Errors
    ///
    /// The build or test output, on failure.
    fn build(&self, app: &GeneratedApp, files: &[SourceFile]) -> Result<(), String>;
}

/// Whether the user has asked for this to stop.
///
/// A trait with one method rather than a channel or a flag, so a caller can
/// wire it to whatever it already has and the loop needs no concurrency
/// machinery of its own.
pub trait Cancellation {
    /// Whether to stop.
    fn is_cancelled(&self) -> bool;
}

/// A run that cannot be cancelled, for callers that have nothing to cancel it
/// with.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancelled;

impl Cancellation for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// What one attempt at building produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    /// Which attempt this was. Zero is the first generation.
    pub attempt: u32,

    /// What the model said it was fixing, when this round was a repair.
    pub diagnosis: Option<String>,

    /// What the build said, when it failed.
    pub failure: Option<String>,
}

impl Round {
    /// Whether this round produced something that worked.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
    }
}

/// Everything one generation run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The application, as it ended up.
    pub app: GeneratedApp,

    /// Its files, after every repair that was applied.
    pub files: Vec<SourceFile>,

    /// What happened, round by round.
    ///
    /// Kept in full rather than reduced to a success flag: "it worked on the
    /// third attempt, and here is what was wrong with the first two" is what a
    /// person wants to know, and what the audit record should say.
    pub rounds: Vec<Round>,

    /// What the whole run cost.
    pub usage: Usage,
}

impl Outcome {
    /// How many repairs were needed.
    #[must_use]
    pub fn repairs(&self) -> usize {
        self.rounds.len().saturating_sub(1)
    }

    /// The recipe this run produced, for the version digest.
    #[must_use]
    pub fn recipe(&self, limits: &str) -> Recipe {
        let mut app = self.app.clone();
        app.files.clone_from(&self.files);
        app.recipe(limits)
    }

    /// How this run is described to a person.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.repairs() {
            0 => format!("Built first time, {}.", self.usage.describe()),
            1 => format!("Built after one fix, {}.", self.usage.describe()),
            n => format!("Built after {n} fixes, {}.", self.usage.describe()),
        }
    }
}

/// How long the loop has been running, supplied by the caller.
///
/// A trait rather than reading the clock, so a test can assert the wall-clock
/// ceiling without waiting for it to elapse.
pub trait Elapsed {
    /// Seconds since the run started.
    fn seconds(&self) -> i64;
}

/// An elapsed clock that reads the real time.
#[derive(Debug, Clone, Copy)]
pub struct RealClock {
    started: ephemeral_core::Timestamp,
}

impl Default for RealClock {
    fn default() -> Self {
        Self::start()
    }
}

impl RealClock {
    /// Starts the clock now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            started: ephemeral_core::now(),
        }
    }
}

impl Elapsed for RealClock {
    fn seconds(&self) -> i64 {
        (ephemeral_core::now() - self.started).num_seconds()
    }
}

/// Everything the loop needs that is not the provider.
pub struct Run<'a> {
    /// What may be spent.
    pub budget: &'a GenerationBudget,

    /// What turns source into something that runs.
    pub builder: &'a dyn Builder,

    /// Whether the user has stopped it.
    pub cancellation: &'a dyn Cancellation,

    /// How long it has been going.
    pub clock: &'a dyn Elapsed,
}

/// Plans, generates, builds, and repairs until it works or a bound is reached.
///
/// # Errors
///
/// [`AgentError::BudgetExhausted`] when a ceiling is reached — which is a bound
/// doing its job rather than a failure — [`AgentError::Cancelled`] when the user
/// stops it, and the provider's own errors otherwise.
pub fn generate(
    provider: &dyn AgentProvider,
    intent: &str,
    run: &Run<'_>,
) -> Result<Outcome, AgentError> {
    let mut usage = Usage::default();

    check(run, usage)?;
    let planned = provider.plan(intent)?;
    usage = usage.plus(planned.usage);
    planned.result.validate()?;

    check(run, usage)?;
    let generated = provider.generate(&planned.result)?;
    usage = usage.plus(generated.usage);
    let app = generated.result;
    app.validate()?;

    let mut files = app.files.clone();
    let mut rounds = Vec::new();

    // Attempt 0 is the original generation; every attempt after it is a repair,
    // which is why the ceiling is compared against `rounds.len()` rather than
    // the loop counter.
    loop {
        check(run, usage)?;

        let attempt = u32::try_from(rounds.len()).unwrap_or(u32::MAX);
        let outcome = run.builder.build(&app, &files);

        match outcome {
            Ok(()) => {
                rounds.push(Round {
                    attempt,
                    diagnosis: rounds
                        .last()
                        .and_then(|last: &Round| last.diagnosis.clone()),
                    failure: None,
                });

                return Ok(Outcome {
                    app,
                    files,
                    rounds,
                    usage,
                });
            }
            Err(failure) => {
                rounds.push(Round {
                    attempt,
                    diagnosis: None,
                    failure: Some(failure.clone()),
                });

                // The ceiling is on *repairs*, so the first failure has not
                // used one yet. Comparing before asking for a repair is what
                // makes `max_repairs: 0` mean "do not repair" rather than
                // "repair once".
                let repairs_used =
                    u32::try_from(rounds.len().saturating_sub(1)).unwrap_or(u32::MAX);
                if repairs_used >= run.budget.max_repairs {
                    return Err(AgentError::BudgetExhausted {
                        what: "repair attempts".to_owned(),
                        limit: run.budget.max_repairs.to_string(),
                    });
                }

                check(run, usage)?;
                let repair = provider.repair(&app, &files, &failure)?;
                usage = usage.plus(repair.usage);

                files = repair.result.applied_to(&files);

                // Recorded against the round that will use it, so the history
                // reads "this failed, so it tried this".
                if let Some(last) = rounds.last_mut() {
                    last.diagnosis = Some(repair.result.diagnosis.clone());
                }
            }
        }
    }
}

/// Stops the run if a bound has been reached.
fn check(run: &Run<'_>, usage: Usage) -> Result<(), AgentError> {
    if run.cancellation.is_cancelled() {
        return Err(AgentError::Cancelled);
    }

    let elapsed = run.clock.seconds();
    let allowed = run.budget.max_duration.as_seconds();
    if elapsed > allowed {
        return Err(AgentError::BudgetExhausted {
            what: "the time allowed for this build".to_owned(),
            limit: run.budget.max_duration.describe(),
        });
    }

    if usage.exceeds(run.budget) {
        return Err(AgentError::BudgetExhausted {
            what: "the amount this build may cost".to_owned(),
            limit: run.budget.max_spend_cents.map_or_else(
                || "no ceiling".to_owned(),
                |cents| format!("${:.2}", f64::from(cents) / 100.0),
            ),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::mock::{Behaviour, MockProvider};
    use ephemeral_core::retention::RetentionPeriod;

    /// A builder that fails a fixed number of times, then succeeds.
    struct FailsTimes {
        remaining: Cell<u32>,
    }

    impl FailsTimes {
        fn new(times: u32) -> Self {
            Self {
                remaining: Cell::new(times),
            }
        }
    }

    impl Builder for FailsTimes {
        fn build(&self, _app: &GeneratedApp, _files: &[SourceFile]) -> Result<(), String> {
            let left = self.remaining.get();
            if left == 0 {
                return Ok(());
            }
            self.remaining.set(left - 1);
            Err("SyntaxError: invalid syntax\n".to_owned())
        }
    }

    struct AlwaysFails;

    impl Builder for AlwaysFails {
        fn build(&self, _app: &GeneratedApp, _files: &[SourceFile]) -> Result<(), String> {
            Err("still broken\n".to_owned())
        }
    }

    struct Stopped;

    impl Cancellation for Stopped {
        fn is_cancelled(&self) -> bool {
            true
        }
    }

    struct FixedClock(i64);

    impl Elapsed for FixedClock {
        fn seconds(&self) -> i64 {
            self.0
        }
    }

    fn run<'a>(
        budget: &'a GenerationBudget,
        builder: &'a dyn Builder,
        clock: &'a dyn Elapsed,
    ) -> Run<'a> {
        Run {
            budget,
            builder,
            cancellation: &NeverCancelled,
            clock,
        }
    }

    #[test]
    fn a_build_that_works_first_time_needs_no_repairs() {
        let budget = GenerationBudget::default();
        let builder = FailsTimes::new(0);
        let clock = FixedClock(0);

        let outcome = generate(
            &MockProvider::new(),
            "compare two CSV files",
            &run(&budget, &builder, &clock),
        )
        .unwrap();

        assert_eq!(outcome.repairs(), 0);
        assert_eq!(outcome.rounds.len(), 1);
        assert!(outcome.rounds[0].succeeded());
        assert!(outcome.describe().contains("first time"));
    }

    /// The path that matters: broken, diagnosed, fixed, built.
    #[test]
    fn a_build_that_fails_once_is_repaired_and_succeeds() {
        let budget = GenerationBudget::default();
        let builder = FailsTimes::new(1);
        let clock = FixedClock(0);
        let provider = MockProvider::with(Behaviour::FailsThenRepairs);

        let outcome = generate(
            &provider,
            "compare two CSV files",
            &run(&budget, &builder, &clock),
        )
        .unwrap();

        assert_eq!(outcome.repairs(), 1);
        assert_eq!(provider.repairs(), 1);
        assert!(!outcome.rounds[0].succeeded());
        assert!(
            outcome.rounds[0].diagnosis.is_some(),
            "a failed round should record what was tried next"
        );
        assert!(outcome.rounds[1].succeeded());
    }

    /// The bound that stops an agent looping forever on something it cannot
    /// fix. Without it, `NeverRepairs` would run until the process was killed.
    #[test]
    fn the_repair_budget_is_a_ceiling_rather_than_a_suggestion() {
        let budget = GenerationBudget {
            max_repairs: 2,
            ..GenerationBudget::default()
        };
        let builder = AlwaysFails;
        let clock = FixedClock(0);
        let provider = MockProvider::with(Behaviour::NeverRepairs);

        let error = generate(&provider, "x", &run(&budget, &builder, &clock)).unwrap_err();

        assert!(
            matches!(error, AgentError::BudgetExhausted { .. }),
            "{error:?}"
        );
        assert_eq!(provider.repairs(), 2, "it must not exceed the budget");
    }

    /// A budget of zero repairs means do not repair, not repair once.
    #[test]
    fn a_repair_budget_of_zero_does_not_repair_at_all() {
        let budget = GenerationBudget {
            max_repairs: 0,
            ..GenerationBudget::default()
        };
        let builder = AlwaysFails;
        let clock = FixedClock(0);
        let provider = MockProvider::with(Behaviour::NeverRepairs);

        let error = generate(&provider, "x", &run(&budget, &builder, &clock)).unwrap_err();

        assert!(matches!(error, AgentError::BudgetExhausted { .. }));
        assert_eq!(provider.repairs(), 0);
    }

    /// A run that hangs has to end, and the clock is a parameter so testing
    /// that needs no waiting.
    #[test]
    fn a_run_that_takes_too_long_is_stopped() {
        let budget = GenerationBudget {
            max_duration: RetentionPeriod::seconds(60),
            ..GenerationBudget::default()
        };
        let builder = FailsTimes::new(0);
        let clock = FixedClock(61);

        let error =
            generate(&MockProvider::new(), "x", &run(&budget, &builder, &clock)).unwrap_err();

        let AgentError::BudgetExhausted { what, .. } = &error else {
            panic!("expected a budget error, got {error:?}");
        };
        assert!(what.contains("time"), "{what}");
    }

    /// A surprise on a card is a security and financial issue, not a quality
    /// one.
    #[test]
    fn a_run_that_costs_too_much_is_stopped() {
        let budget = GenerationBudget {
            // The mock charges 2 cents a call, so this is exceeded partway in.
            max_spend_cents: Some(1),
            ..GenerationBudget::default()
        };
        let builder = FailsTimes::new(0);
        let clock = FixedClock(0);

        let error =
            generate(&MockProvider::new(), "x", &run(&budget, &builder, &clock)).unwrap_err();

        let AgentError::BudgetExhausted { what, .. } = &error else {
            panic!("expected a budget error, got {error:?}");
        };
        assert!(what.contains("cost"), "{what}");
    }

    /// The user can stop it, and stopping is not a failure.
    #[test]
    fn a_cancelled_run_stops_immediately() {
        let budget = GenerationBudget::default();
        let builder = FailsTimes::new(0);
        let clock = FixedClock(0);
        let provider = MockProvider::new();

        let error = generate(
            &provider,
            "x",
            &Run {
                budget: &budget,
                builder: &builder,
                cancellation: &Stopped,
                clock: &clock,
            },
        )
        .unwrap_err();

        assert!(matches!(error, AgentError::Cancelled));
        assert_eq!(
            provider.generations(),
            0,
            "cancelling should stop before doing more work"
        );
    }

    /// A plan Ephemeral will not act on stops the run before anything is
    /// generated, rather than after.
    #[test]
    fn an_invalid_plan_stops_before_generating() {
        let budget = GenerationBudget::default();
        let builder = FailsTimes::new(0);
        let clock = FixedClock(0);
        let provider = MockProvider::with(Behaviour::ProducesAnInvalidPlan);

        let error = generate(&provider, "x", &run(&budget, &builder, &clock)).unwrap_err();

        assert!(matches!(error, AgentError::Refused(_)), "{error:?}");
        assert_eq!(provider.generations(), 0);
    }

    /// Every call's cost is counted, including the ones inside repairs.
    #[test]
    fn the_whole_run_is_costed_including_repairs() {
        let budget = GenerationBudget::default();
        let builder = FailsTimes::new(2);
        let clock = FixedClock(0);

        let outcome = generate(
            &MockProvider::with(Behaviour::FailsThenRepairs),
            "x",
            &run(&budget, &builder, &clock),
        )
        .unwrap();

        // plan + generate + two repairs, at 2 cents each.
        assert_eq!(outcome.usage.cents, 8);
        assert_eq!(outcome.repairs(), 2);
    }

    /// The version digest has to reflect the repaired source, not what was
    /// first generated — otherwise a repaired application claims to be the
    /// broken one.
    #[test]
    fn the_recipe_reflects_the_repaired_source() {
        let budget = GenerationBudget::default();
        let clock = FixedClock(0);

        let clean = generate(
            &MockProvider::new(),
            "x",
            &run(&budget, &FailsTimes::new(0), &clock),
        )
        .unwrap();

        let repaired = generate(
            &MockProvider::with(Behaviour::FailsThenRepairs),
            "x",
            &run(&budget, &FailsTimes::new(1), &clock),
        )
        .unwrap();

        // Both end at the mock's working source, so the recipes agree — which
        // is the point: identity follows the content, not the route taken.
        assert_eq!(clean.recipe("cpu=500"), repaired.recipe("cpu=500"));
        assert_eq!(repaired.files, clean.files);
    }
}
