//! The reference WebAssembly application actually runs.
//!
//! Everything else that exercises this runtime assembles a module out of
//! WebAssembly text. That proves the sandbox holds; it does not prove somebody
//! could *write* something for it. The difference is the one
//! `ephemeral-agent`'s `reference_app` test exists for: "the fixture is a
//! plausible string" against "the fixture is an application".
//!
//! So this compiles `examples/tally` — ordinary Rust, no dependencies — for
//! `wasm32-wasip1`, and runs the result through exactly the sandbox any other
//! application gets. Both tiers, in one program: it takes the arguments a form
//! composed, and with `--format html` it writes a page for a host to render.
//!
//! Skips itself, **loudly**, when the target is not installed. A test that
//! quietly passes when it did not run is worse than no test.

#![cfg(feature = "wasm")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use ephemeral_core::{AppId, AppManifest, manifest::RuntimeSpec, storage::StorageLayout};
use ephemeral_runtime::wasm::{HANDHELD_CEILING, Ran, Runnable, Shown, run_application};

/// The reference application, compiled, or `None` if this machine cannot.
///
/// Built into a target directory of its own. Sharing the one this test is
/// running out of would deadlock on the lock cargo takes, which is a
/// spectacular way to make a suite hang with no output at all.
fn compiled() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/tally");
    let into = std::env::temp_dir().join("ephemeral-reference-wasm");

    let built = Command::new(std::env::var("CARGO").as_deref().unwrap_or("cargo"))
        .args(["build", "--release", "--target", "wasm32-wasip1"])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", &into)
        .output()
        .ok()?;

    if !built.status.success() {
        let said = String::from_utf8_lossy(&built.stderr);
        // The one failure that is this machine's rather than the code's.
        if said.contains("wasm32-wasip1") {
            println!(
                "(skipping the reference application: no wasm32-wasip1 target here — \
                 `rustup target add wasm32-wasip1`)"
            );
            return None;
        }
        panic!("the reference application did not build:\n{said}");
    }

    Some(into.join("wasm32-wasip1/release/tally.wasm"))
}

/// Installs the reference application, with a CSV in its own storage.
fn installed(home: &Path, module: &Path, page: bool) -> (StorageLayout, AppManifest) {
    let app = AppId::parse("tally").unwrap();
    let layout = StorageLayout::new(home);

    std::fs::create_dir_all(layout.app(&app).source()).unwrap();
    std::fs::copy(module, layout.app(&app).source().join("program.wasm")).unwrap();

    std::fs::create_dir_all(layout.app(&app).data()).unwrap();
    std::fs::write(
        layout.app(&app).data().join("files.csv"),
        "name,size,owner\nreport.csv,1024,ana\nnotes.txt,88,bo\n\narchive.zip,90210,cy\n",
    )
    .unwrap();

    let mut runtime = RuntimeSpec::wasm_job("program.wasm", Vec::new());
    if page {
        runtime.interface = ephemeral_core::manifest::AppInterface::Web;
    }

    let mut manifest = AppManifest::requested(app, "Tally");
    manifest.runtime = Some(runtime);

    (layout, manifest)
}

fn ran(home: &Path, module: &Path, page: bool, arguments: &[&str]) -> Ran {
    let (layout, manifest) = installed(home, module, page);

    run_application(&Runnable {
        manifest: &manifest,
        layout: &layout,
        granted: &[],
        home: home.to_path_buf(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        ceiling: HANDHELD_CEILING,
    })
    .expect("the reference application runs")
}

/// **Tier two, with a real program.** A form's answers become an argument
/// vector, and the application does what they say.
#[test]
fn the_reference_application_counts_what_it_was_given() {
    let Some(module) = compiled() else { return };
    let home = tempfile::tempdir().unwrap();

    let outcome = ran(home.path(), &module, false, &["--file", "/data/files.csv"]);

    assert!(outcome.completed.succeeded, "{}", outcome.completed.output);
    assert!(
        outcome.completed.output.contains("3 rows, 3 columns"),
        "three rows under a header, and a blank line that is not one: {}",
        outcome.completed.output
    );
    assert_eq!(outcome.shown, Shown::Text);
}

/// The flag a form draws as a checkbox reaches the program as a flag, and
/// changes the answer. A declaration nothing acts on is a form that lies.
#[test]
fn a_flag_from_the_form_changes_what_it_counts() {
    let Some(module) = compiled() else { return };
    let home = tempfile::tempdir().unwrap();

    let outcome = ran(
        home.path(),
        &module,
        false,
        &["--file", "/data/files.csv", "--no-headers"],
    );

    assert!(
        outcome.completed.output.contains("4 rows"),
        "without a header row there is one more: {}",
        outcome.completed.output
    );
}

/// **Tier one, with a real program.** It writes a page, and the host is told
/// to render it — no socket, no server, and so no network permission.
#[test]
fn the_reference_application_can_write_a_page() {
    let Some(module) = compiled() else { return };
    let home = tempfile::tempdir().unwrap();

    let outcome = ran(
        home.path(),
        &module,
        true,
        &["--file", "/data/files.csv", "--format", "html"],
    );

    assert!(outcome.completed.succeeded);
    assert_eq!(outcome.shown, Shown::Page);
    assert!(outcome.completed.output.contains("<dt>Rows</dt><dd>3</dd>"));

    // Nothing fetched from anywhere. A host renders this with every subresource
    // blocked, so a page that needed one would render as a broken version of
    // itself — which is a thing to find here rather than on somebody's phone.
    for reaching in ["http://", "https://", "<script"] {
        assert!(
            !outcome.completed.output.contains(reaching),
            "the page reaches for {reaching}: {}",
            outcome.completed.output
        );
    }
}

/// A file it was not given is a refusal it can explain, not a crash. This is
/// the sandbox from the *application's* side — the message a person actually
/// sees when a grant is missing.
#[test]
fn a_file_it_was_not_given_is_an_error_it_can_explain() {
    let Some(module) = compiled() else { return };
    let home = tempfile::tempdir().unwrap();

    let outcome = ran(home.path(), &module, false, &["--file", "/etc/passwd"]);

    assert!(!outcome.completed.succeeded);
    // Its own code. 124, 137 and 134 are the ones Ephemeral reports when it
    // stopped something, so 1 is the program saying it failed rather than the
    // sandbox saying it was ended.
    assert_eq!(outcome.completed.exit_code, 1);
    assert!(
        outcome
            .completed
            .output
            .contains("may not have been allowed"),
        "and it says the likeliest reason: {}",
        outcome.completed.output
    );
}

/// A real program is bounded like one. This asserts the reference application
/// finishes inside a *second* of allowance, where a handset gets thirty — so
/// the fuel conversion is not merely generous in theory.
#[test]
fn the_reference_application_finishes_inside_a_second_of_allowance() {
    let Some(module) = compiled() else { return };
    let home = tempfile::tempdir().unwrap();

    let (layout, manifest) = installed(home.path(), &module, false);
    let outcome = run_application(&Runnable {
        manifest: &manifest,
        layout: &layout,
        granted: &[],
        home: home.path().to_path_buf(),
        arguments: vec!["--file".to_owned(), "/data/files.csv".to_owned()],
        ceiling: Duration::from_secs(1),
    })
    .expect("it runs");

    assert!(
        outcome.completed.succeeded,
        "a second of allowance was not enough, so the conversion is too tight: {}",
        outcome.completed.output
    );
    assert_eq!(outcome.completed.exit_code, 0, "and nothing stopped it");
}
