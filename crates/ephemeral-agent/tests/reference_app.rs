//! The reference application actually works.
//!
//! Everything else about the mock provider tests that its output is *well
//! formed* — the paths are safe, the digests are stable, the plan validates.
//! None of that would notice if the Python it produces did not run.
//!
//! This test writes the reference application to a temporary directory and runs
//! its own test suite with a real interpreter. It is the difference between
//! "the fixture is a plausible string" and "the fixture is an application",
//! and it matters because `--provider mock` is what somebody gets when they try
//! Ephemeral without a model.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

use ephemeral_agent::{AgentProvider, MockProvider};

/// The interpreter to test with, if this machine has one.
///
/// Tried in order, because the name differs between platforms. Returns `None`
/// rather than guessing, and the caller says out loud that it skipped —
/// a test that quietly passes when it did not run is worse than no test.
fn interpreter() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

/// Writes the reference application to `dir` and returns its test command.
fn write_reference_app(dir: &Path) -> Vec<String> {
    let provider = MockProvider::new();
    let plan = provider
        .plan("compare these two CSV files and show me what's different")
        .expect("the mock plans")
        .result;
    let app = provider.generate(&plan).expect("the mock generates").result;

    app.validate()
        .expect("what the mock produces must validate");

    for file in &app.files {
        std::fs::write(dir.join(&file.path), &file.contents).expect("writing generated source");
    }
    std::fs::write(dir.join("Dockerfile"), &app.dockerfile).expect("writing the Dockerfile");

    app.test_command
}

/// The application's own tests pass. If they do not, the mock is producing
/// something that would fail validation the moment a real build ran it.
#[test]
fn the_reference_applications_own_tests_pass() {
    let Some(python) = interpreter() else {
        println!("skipped: no Python interpreter on this machine");
        return;
    };

    let dir = tempfile::tempdir().expect("a temporary directory");
    let test_command = write_reference_app(dir.path());

    // The command the provider declared, not one invented here: if the two ever
    // disagree, this test is verifying something Ephemeral would never run.
    assert_eq!(test_command.first().map(String::as_str), Some("python"));

    let output = Command::new(python)
        .args(&test_command[1..])
        .current_dir(dir.path())
        .output()
        .expect("running the generated tests");

    assert!(
        output.status.success(),
        "the reference application's tests failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// It compares two real files and reports what actually differs.
#[test]
fn the_reference_application_compares_two_files() {
    let Some(python) = interpreter() else {
        println!("skipped: no Python interpreter on this machine");
        return;
    };

    let dir = tempfile::tempdir().expect("a temporary directory");
    write_reference_app(dir.path());

    std::fs::write(dir.path().join("left.csv"), "id,price\na,100\nb,200\n").unwrap();
    std::fs::write(dir.path().join("right.csv"), "id,price\na,150\nc,300\n").unwrap();

    let output = Command::new(python)
        .args(["compare.py", "left.csv", "right.csv"])
        .current_dir(dir.path())
        .output()
        .expect("running the comparator");

    let printed = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{printed}");
    assert!(printed.contains("added:   c"), "{printed}");
    assert!(printed.contains("removed: b"), "{printed}");
    assert!(printed.contains("changed: a"), "{printed}");
}

/// Two identical files differ in nothing, and it says so rather than printing
/// an empty result somebody has to interpret.
#[test]
fn identical_files_are_reported_as_identical() {
    let Some(python) = interpreter() else {
        println!("skipped: no Python interpreter on this machine");
        return;
    };

    let dir = tempfile::tempdir().expect("a temporary directory");
    write_reference_app(dir.path());

    std::fs::write(dir.path().join("a.csv"), "id,price\na,100\n").unwrap();
    std::fs::write(dir.path().join("b.csv"), "id,price\na,100\n").unwrap();

    let output = Command::new(python)
        .args(["compare.py", "a.csv", "b.csv"])
        .current_dir(dir.path())
        .output()
        .expect("running the comparator");

    assert!(
        String::from_utf8_lossy(&output.stdout).contains("same rows"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// The build recipe needs no network. An application whose build had to fetch
/// something could not be built in a container that has no network — which is
/// the default sandbox.
#[test]
fn the_reference_application_needs_nothing_from_the_network_to_build() {
    let provider = MockProvider::new();
    let plan = provider.plan("compare two CSV files").unwrap().result;
    let app = provider.generate(&plan).unwrap().result;

    let recipe = app.dockerfile.to_lowercase();
    for fetcher in ["pip install", "apt-get", "npm install", "curl", "wget"] {
        assert!(
            !recipe.contains(fetcher),
            "the Dockerfile runs `{fetcher}`, which a network-less build cannot do:\n{}",
            app.dockerfile
        );
    }
}
