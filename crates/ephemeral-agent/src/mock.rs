//! A provider that returns the same thing every time.
//!
//! Not a stub. This is the implementation every test in the product runs
//! against, including the end-to-end ones, because the build-and-repair loop is
//! the most intricate control flow Ephemeral has and it cannot be tested against
//! something non-deterministic ([ADR-0008]).
//!
//! It is also genuinely useful outside tests: `--provider mock` gives somebody a
//! working application without a credential, a network connection, or a bill,
//! which is the difference between trying Ephemeral and reading about it.
//!
//! ## What it can be told to do
//!
//! A mock that only ever succeeds tests nothing interesting. This one can be
//! told to fail in each of the ways a real provider fails — unavailable, an
//! unreadable response, a plan that will not validate, a first build that
//! breaks and a repair that fixes it — so the loop's error paths are exercised
//! rather than assumed.
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md

use std::cell::Cell;

use ephemeral_core::{
    manifest::{AppInterface, RuntimeKind},
    permission::{AppPermission, PathScope},
};

use crate::{
    plan::{GeneratedApp, PermissionRequest, Plan, RepairAttempt, SourceFile},
    provider::{AgentError, AgentProvider, Attempt, Usage},
};

/// The image the mock builds on.
const IMAGE: &str = "python:3.12-slim";

/// How the mock is told to misbehave.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Behaviour {
    /// Produces a working application first time.
    #[default]
    Succeeds,

    /// Produces an application whose first build fails, and repairs it when
    /// asked.
    ///
    /// The interesting case: it exercises the whole plan → generate → build →
    /// fail → repair → build → succeed path that nothing else covers.
    FailsThenRepairs,

    /// Produces an application that never builds, however many repairs are
    /// attempted.
    ///
    /// Tests that the repair budget is a real ceiling rather than a suggestion.
    NeverRepairs,

    /// Cannot be used at all.
    Unavailable,

    /// Returns a plan Ephemeral refuses to act on.
    ///
    /// Specifically: a permission request with no stated reason, which cannot
    /// honestly be put to a person as a question.
    ProducesAnInvalidPlan,

    /// Returns something structurally unreadable.
    ReturnsGarbage,
}

/// A deterministic provider.
///
/// Interior mutability rather than `&mut self` on the trait: a provider is
/// shared, and threading mutability through the generation loop for the benefit
/// of a test double would shape the real interface around the fake one.
#[derive(Debug)]
pub struct MockProvider {
    behaviour: Behaviour,
    generations: Cell<u32>,
    repairs: Cell<u32>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    /// A provider that produces a working application.
    #[must_use]
    pub fn new() -> Self {
        Self::with(Behaviour::Succeeds)
    }

    /// A provider that behaves in a particular way.
    #[must_use]
    pub fn with(behaviour: Behaviour) -> Self {
        Self {
            behaviour,
            generations: Cell::new(0),
            repairs: Cell::new(0),
        }
    }

    /// How many times it has been asked to generate.
    #[must_use]
    pub fn generations(&self) -> u32 {
        self.generations.get()
    }

    /// How many times it has been asked to repair.
    #[must_use]
    pub fn repairs(&self) -> u32 {
        self.repairs.get()
    }

    /// The source of an application that works.
    fn working_source() -> Vec<SourceFile> {
        vec![
            SourceFile::new(
                "main.py",
                "import sys\n\
                 \n\
                 def compare(left, right):\n\
                 \x20   return [line for line in left if line not in right]\n\
                 \n\
                 if __name__ == '__main__':\n\
                 \x20   print('ready')\n",
            ),
            SourceFile::new(
                "test_main.py",
                "from main import compare\n\
                 \n\
                 def test_compare():\n\
                 \x20   assert compare(['a'], []) == ['a']\n",
            ),
        ]
    }

    /// The source of an application that does not build.
    ///
    /// A syntax error rather than a logic error, so a real build would fail on
    /// it too and the fixture stays honest if it is ever run for real.
    fn broken_source() -> Vec<SourceFile> {
        vec![SourceFile::new(
            "main.py",
            "def compare(left, right)\n\x20   return left\n",
        )]
    }

    /// A fixed cost, so a test asserting on budgets has something to assert on.
    fn cost() -> Usage {
        Usage {
            input_tokens: 1200,
            output_tokens: 400,
            cents: 2,
        }
    }
}

impl AgentProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn availability(&self) -> Result<(), AgentError> {
        if self.behaviour == Behaviour::Unavailable {
            return Err(AgentError::Unavailable {
                provider: "mock".to_owned(),
                reason: "it was configured to be unavailable, which is what you asked for"
                    .to_owned(),
            });
        }

        Ok(())
    }

    fn plan(&self, intent: &str) -> Result<Attempt<Plan>, AgentError> {
        self.availability()?;

        if self.behaviour == Behaviour::ReturnsGarbage {
            return Err(AgentError::Unreadable {
                provider: "mock".to_owned(),
                reason: "the response was not a plan".to_owned(),
                raw: "{{ this is not json".to_owned(),
            });
        }

        // Echoing the intent is what makes the mock useful rather than merely
        // deterministic: a user running `--provider mock` sees their own words
        // come back, so it is obvious the plan is a fixture and not a model's
        // reading of what they asked for.
        let reason = if self.behaviour == Behaviour::ProducesAnInvalidPlan {
            String::new()
        } else {
            "to read the files you want compared".to_owned()
        };

        Ok(Attempt::new(
            Plan {
                summary: format!("A fixed example application, standing in for: {intent}"),
                interface: AppInterface::CommandLine,
                runtime: RuntimeKind::Docker,
                image: IMAGE.to_owned(),
                // A constant the parser accepts, so an unparseable one would
                // be a bug in this file rather than anything a caller did. It
                // still degrades to "asks for nothing" rather than panicking,
                // because a test double that can bring down the process is a
                // worse test double.
                requests: PathScope::parse("~/Downloads/**").map_or_else(
                    |_| Vec::new(),
                    |scope| {
                        vec![PermissionRequest {
                            permission: AppPermission::read(scope),
                            reason,
                        }]
                    },
                ),
            },
            Self::cost(),
        ))
    }

    fn generate(&self, plan: &Plan) -> Result<Attempt<GeneratedApp>, AgentError> {
        self.availability()?;
        self.generations.set(self.generations.get() + 1);

        let broken = matches!(
            self.behaviour,
            Behaviour::FailsThenRepairs | Behaviour::NeverRepairs
        );

        Ok(Attempt::new(
            GeneratedApp {
                plan: plan.clone(),
                files: if broken {
                    Self::broken_source()
                } else {
                    Self::working_source()
                },
                dockerfile: format!(
                    "FROM {IMAGE}\nWORKDIR /app\nCOPY . /app\nCMD [\"python\", \"main.py\"]\n"
                ),
                entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
            },
            Self::cost(),
        ))
    }

    fn repair(
        &self,
        _app: &GeneratedApp,
        _files: &[SourceFile],
        failure: &str,
    ) -> Result<Attempt<RepairAttempt>, AgentError> {
        self.availability()?;
        self.repairs.set(self.repairs.get() + 1);

        // `failure` is untrusted input — build output can contain anything a
        // dependency chose to print. The mock reads its length and nothing
        // else, which is exactly as much as it should.
        let diagnosis = format!(
            "the build failed with {} bytes of output; replacing main.py",
            failure.len()
        );

        let files = if self.behaviour == Behaviour::NeverRepairs {
            Self::broken_source()
        } else {
            Self::working_source()
        };

        Ok(Attempt::new(
            RepairAttempt { diagnosis, files },
            Self::cost(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property everything else rests on. A provider that varied would make
    /// every test that uses it flaky.
    #[test]
    fn the_same_intent_produces_the_same_application() {
        let one = MockProvider::new();
        let other = MockProvider::new();

        let plan = one.plan("compare two CSV files").unwrap().result;
        let same = other.plan("compare two CSV files").unwrap().result;
        assert_eq!(plan, same);

        assert_eq!(
            one.generate(&plan).unwrap().result,
            other.generate(&same).unwrap().result
        );
    }

    #[test]
    fn a_different_intent_produces_a_different_plan() {
        let provider = MockProvider::new();

        assert_ne!(
            provider.plan("compare CSV files").unwrap().result,
            provider.plan("rename photos").unwrap().result
        );
    }

    /// What the mock produces has to survive the same validation a real
    /// provider's output does, or it is testing a path nothing else takes.
    #[test]
    fn what_the_mock_produces_passes_validation() {
        let provider = MockProvider::new();
        let plan = provider.plan("compare two CSV files").unwrap().result;

        plan.validate().unwrap();
        provider.generate(&plan).unwrap().result.validate().unwrap();
    }

    /// The interesting case: broken first, fixed on repair.
    #[test]
    fn the_repairing_behaviour_actually_changes_the_source() {
        let provider = MockProvider::with(Behaviour::FailsThenRepairs);
        let plan = provider.plan("compare two CSV files").unwrap().result;
        let app = provider.generate(&plan).unwrap().result;

        let repair = provider
            .repair(&app, &app.files, "SyntaxError: invalid syntax")
            .unwrap()
            .result;

        let repaired = repair.applied_to(&app.files);
        assert_ne!(
            repaired, app.files,
            "a repair that changes nothing is not one"
        );
        assert!(repaired.iter().any(|file| file.path == "test_main.py"));
    }

    /// A repair budget is only a ceiling if something can fail against it
    /// forever.
    #[test]
    fn the_never_repairing_behaviour_stays_broken() {
        let provider = MockProvider::with(Behaviour::NeverRepairs);
        let plan = provider.plan("x").unwrap().result;
        let app = provider.generate(&plan).unwrap().result;

        for _ in 0..5 {
            let repair = provider.repair(&app, &app.files, "still broken").unwrap();
            assert_eq!(repair.result.applied_to(&app.files), app.files);
        }
        assert_eq!(provider.repairs(), 5);
    }

    #[test]
    fn an_unavailable_provider_refuses_everything_with_a_reason() {
        let provider = MockProvider::with(Behaviour::Unavailable);

        assert!(provider.availability().is_err());
        assert!(matches!(
            provider.plan("x").unwrap_err(),
            AgentError::Unavailable { .. }
        ));
    }

    /// The loop has to cope with a plan it will not act on, so the mock has to
    /// be able to produce one.
    #[test]
    fn the_invalid_plan_behaviour_produces_something_validation_rejects() {
        let provider = MockProvider::with(Behaviour::ProducesAnInvalidPlan);
        let plan = provider.plan("x").unwrap().result;

        assert!(plan.validate().is_err());
    }

    #[test]
    fn the_garbage_behaviour_is_unreadable_rather_than_wrong() {
        let provider = MockProvider::with(Behaviour::ReturnsGarbage);

        assert!(matches!(
            provider.plan("x").unwrap_err(),
            AgentError::Unreadable { .. }
        ));
    }

    /// Costs are reported so a budget can be enforced against something.
    #[test]
    fn every_call_reports_what_it_cost() {
        let provider = MockProvider::new();
        let plan = provider.plan("x").unwrap();

        assert!(plan.usage.cents > 0);
        assert!(plan.usage.input_tokens > 0);
    }

    /// The name goes into the audit record, so it must be a name and not
    /// anything else.
    #[test]
    fn the_provider_names_itself_without_naming_a_credential() {
        let name = MockProvider::new().name().to_owned();

        assert_eq!(name, "mock");
        assert!(!name.contains('-') || !name.contains("sk"), "{name}");
    }
}
