//! The whole journey, from a sentence to something that runs.
//!
//! Phase 2's definition of done is that an application can be built from a
//! natural-language request end to end, with CI exercising the journey against
//! the mock provider and never calling a real model. This file is that test.
//!
//! It deliberately does not touch Docker. The build step is a stand-in that
//! records what it was given, which is what lets the journey run on every
//! platform in CI — the real Docker path is covered by the argv tests in
//! `ephemeral-runtime`, which assert the confinement without a daemon.

// An integration test is not compiled under `cfg(test)`, so the workspace's
// ban on panicking constructs applies here as it does to production code. A
// test that cannot assert is not a test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;

use ephemeral_agent::{
    AgentError, Builder, GeneratedApp, MockProvider, RealClock, Run, SourceFile,
    build::NeverCancelled, generate, mock::Behaviour,
};
use ephemeral_core::{
    Actor, AppId, AppManifest,
    lifecycle::{LifecycleEvent, LifecycleState, TransitionRequest},
    manifest::GenerationBudget,
    permission::{AppPermission, PathScope},
};

/// A build step that records what it was asked to build.
struct Recording {
    failures_left: RefCell<u32>,
    builds: RefCell<Vec<Vec<SourceFile>>>,
}

impl Recording {
    fn new(failures: u32) -> Self {
        Self {
            failures_left: RefCell::new(failures),
            builds: RefCell::new(Vec::new()),
        }
    }
}

impl Builder for Recording {
    fn build(&self, _app: &GeneratedApp, files: &[SourceFile]) -> Result<(), String> {
        self.builds.borrow_mut().push(files.to_vec());

        let mut left = self.failures_left.borrow_mut();
        if *left == 0 {
            return Ok(());
        }
        *left -= 1;
        Err("  File \"main.py\", line 1\n    def compare(left, right)\nSyntaxError\n".to_owned())
    }
}

fn budget() -> GenerationBudget {
    GenerationBudget::default()
}

/// A sentence in, an application out — and the manifest that describes it
/// arrives at `Ready` by a route the state machine agrees with.
#[test]
fn a_sentence_becomes_a_ready_application() {
    let builder = Recording::new(0);
    let clock = RealClock::start();
    let budget = budget();

    let outcome = generate(
        &MockProvider::new(),
        "compare these two CSV files and show me what's different",
        &Run {
            budget: &budget,
            builder: &builder,
            cancellation: &NeverCancelled,
            clock: &clock,
        },
    )
    .expect("the mock provider and a cooperative builder should produce an application");

    assert_eq!(outcome.repairs(), 0);
    assert!(!outcome.files.is_empty());
    assert_eq!(builder.builds.borrow().len(), 1);

    // The manifest can be driven from Requested to Ready on the strength of it.
    let mut manifest = AppManifest::requested(
        AppId::parse("csv-comparator").expect("a valid id"),
        "CSV comparator",
    );

    for (event, actor) in [
        (LifecycleEvent::Plan, Actor::Ephemeral),
        (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
    ] {
        manifest
            .apply(TransitionRequest::new(event, actor, "generating"))
            .unwrap_or_else(|error| panic!("{event}: {error}"));
    }

    manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec {
        kind: outcome.app.plan.runtime,
        image: Some(outcome.app.plan.image.clone()),
        version: None,
        entrypoint: outcome.app.entrypoint.clone(),
        interface: outcome.app.plan.interface,
        port: None,
    });

    for (event, actor) in [
        (LifecycleEvent::GenerationCompleted, Actor::Agent),
        (LifecycleEvent::BuildSucceeded, Actor::Runtime),
        (LifecycleEvent::ValidationPassed, Actor::Runtime),
    ] {
        manifest
            .apply(TransitionRequest::new(event, actor, "generating"))
            .unwrap_or_else(|error| panic!("{event}: {error}"));
    }

    assert_eq!(manifest.lifecycle.state(), LifecycleState::Ready);
    assert!(manifest.lifecycle.state().is_runnable());

    manifest.record_version(&outcome.recipe("cpu=500"), "generated with mock");
    assert_eq!(
        manifest.current_version().map(|version| version.sequence),
        Some(1)
    );
}

/// The path that matters most: it breaks, it is diagnosed, it is fixed, and the
/// second build gets different source from the first.
#[test]
fn a_broken_build_is_repaired_and_the_second_attempt_differs() {
    let builder = Recording::new(1);
    let clock = RealClock::start();
    let budget = budget();

    let outcome = generate(
        &MockProvider::with(Behaviour::FailsThenRepairs),
        "compare two CSV files",
        &Run {
            budget: &budget,
            builder: &builder,
            cancellation: &NeverCancelled,
            clock: &clock,
        },
    )
    .expect("one failure is inside the default repair budget");

    assert_eq!(outcome.repairs(), 1);

    let builds = builder.builds.borrow();
    assert_eq!(builds.len(), 2, "it should have tried twice");
    assert_ne!(
        builds[0], builds[1],
        "a repair that hands the builder the same source is not a repair"
    );

    // And the recorded history says what went wrong before it worked.
    assert!(outcome.rounds[0].failure.is_some());
    assert!(outcome.rounds[0].diagnosis.is_some());
    assert!(outcome.rounds[1].succeeded());
}

/// An application that cannot be fixed stops, rather than burning a budget
/// until somebody notices.
#[test]
fn an_unfixable_application_stops_at_its_repair_budget() {
    let builder = Recording::new(u32::MAX);
    let clock = RealClock::start();
    let budget = GenerationBudget {
        max_repairs: 2,
        ..budget()
    };

    let error = generate(
        &MockProvider::with(Behaviour::NeverRepairs),
        "something impossible",
        &Run {
            budget: &budget,
            builder: &builder,
            cancellation: &NeverCancelled,
            clock: &clock,
        },
    )
    .expect_err("it can never succeed");

    assert!(
        matches!(error, AgentError::BudgetExhausted { .. }),
        "{error:?}"
    );

    // Three builds: the original, and one after each of the two repairs.
    assert_eq!(builder.builds.borrow().len(), 3);
}

/// The generated application asks for something, and what it asks for survives
/// into the version record so a later update can be compared against it.
#[test]
fn what_an_application_asks_for_is_recorded_with_its_version() {
    let builder = Recording::new(0);
    let clock = RealClock::start();
    let budget = budget();

    let outcome = generate(
        &MockProvider::new(),
        "compare two CSV files",
        &Run {
            budget: &budget,
            builder: &builder,
            cancellation: &NeverCancelled,
            clock: &clock,
        },
    )
    .expect("a working application");

    let mut manifest = AppManifest::requested(
        AppId::parse("csv-comparator").expect("a valid id"),
        "CSV comparator",
    );
    manifest.record_version(&outcome.recipe("cpu=500"), "generated");

    let version = manifest.current_version().expect("a version was recorded");
    assert!(
        version.requests.contains(&AppPermission::read(
            PathScope::parse("~/Downloads/**").expect("a valid scope")
        )),
        "the version has to record what it wanted, or nothing can compare it later"
    );

    // Nothing about generating granted anything.
    assert!(
        manifest.permissions.capabilities().is_empty(),
        "generation must not write to the manifest's permission block by itself"
    );
}

/// Two runs of a deterministic provider produce the same application. Without
/// this the digest would be a timestamp wearing an identity's clothes.
#[test]
fn the_journey_is_reproducible() {
    let clock = RealClock::start();
    let budget = budget();

    let run_once = || {
        let builder = Recording::new(0);
        generate(
            &MockProvider::new(),
            "compare two CSV files",
            &Run {
                budget: &budget,
                builder: &builder,
                cancellation: &NeverCancelled,
                clock: &clock,
            },
        )
        .expect("a working application")
    };

    assert_eq!(run_once().recipe("cpu=500"), run_once().recipe("cpu=500"));
}

/// A repaired application and a clean one that end at the same source are the
/// same application. Identity follows content, not the route taken to it.
#[test]
fn how_an_application_was_reached_is_not_part_of_its_identity() {
    let clock = RealClock::start();
    let budget = budget();

    let clean = {
        let builder = Recording::new(0);
        generate(
            &MockProvider::new(),
            "compare two CSV files",
            &Run {
                budget: &budget,
                builder: &builder,
                cancellation: &NeverCancelled,
                clock: &clock,
            },
        )
        .expect("a working application")
    };

    let repaired = {
        let builder = Recording::new(1);
        generate(
            &MockProvider::with(Behaviour::FailsThenRepairs),
            "compare two CSV files",
            &Run {
                budget: &budget,
                builder: &builder,
                cancellation: &NeverCancelled,
                clock: &clock,
            },
        )
        .expect("a working application")
    };

    assert_eq!(repaired.repairs(), 1);
    assert_eq!(clean.repairs(), 0);
    assert_eq!(clean.recipe("cpu=500"), repaired.recipe("cpu=500"));
}
