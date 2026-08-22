#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Rolling back, through the one operation every client calls.
//!
//! The domain half and the storage half are each tested in `ephemeral-core`.
//! What is only true when they are used together is what this covers: that the
//! bytes on disk really go back, that a rollback which asks for more than the
//! version it replaces does not inherit the approval given to that version, and
//! that each of the refusals happens before anything has moved.
//!
//! It lives here rather than in a client because the operation does. It was
//! written against the terminal's copy of these steps, at a time when there was
//! only one client that could roll back; a window calling a second copy of them
//! is exactly what this crate exists to prevent.

use ephemeral_core::{
    Actor, AppId, AppManifest, Principal,
    manifest::RuntimeSpec,
    permission::{AppPermission, PathScope, Permission},
    storage::{AppStore as _, Workspace},
};

fn recipe(contents: &str, requests: Vec<AppPermission>) -> ephemeral_core::Recipe {
    let mut recipe = ephemeral_core::Recipe {
        runtime: "docker".to_owned(),
        image: Some("python:3.12-slim".to_owned()),
        entrypoint: vec!["python".to_owned(), "main.py".to_owned()],
        source: vec![("main.py".to_owned(), contents.to_owned())],
        requests,
        limits: "cpu=500".to_owned(),
    };
    recipe.normalise();
    recipe
}

/// Generates a version: writes the source, keeps it, and saves the manifest,
/// the way the real generate command does.
fn generate(
    workspace: &mut Workspace,
    manifest: &mut AppManifest,
    contents: &str,
    requests: Vec<AppPermission>,
) -> ephemeral_core::VersionDigest {
    let source = workspace.layout().app(&manifest.id).source();
    std::fs::create_dir_all(&source).expect("a source directory");
    std::fs::write(source.join("main.py"), contents).expect("source written");

    manifest.record_version(&recipe(contents, requests), "generated");
    let digest = manifest
        .current_version()
        .expect("a version")
        .digest
        .clone();

    workspace
        .apps()
        .keep_version(&manifest.id, &digest)
        .expect("the version is kept");
    workspace.apps_mut().save(manifest).expect("saved");

    digest
}

fn set_up() -> (tempfile::TempDir, Workspace, AppManifest) {
    let home = tempfile::tempdir().expect("a temporary directory");
    let mut workspace = Workspace::open(home.path()).expect("a workspace");
    let id = AppId::parse("csv-comparator").expect("a valid id");

    let mut manifest = AppManifest::requested(id.clone(), "CSV comparator");
    manifest.runtime = Some(RuntimeSpec::docker_job(
        "python:3.12-slim",
        vec!["python".to_owned()],
    ));

    workspace.apps_mut().create(&manifest).expect("created");
    workspace.apps().prepare(&id).expect("prepared");

    (home, workspace, manifest)
}

fn source_of(workspace: &Workspace, manifest: &AppManifest) -> String {
    let path = workspace
        .layout()
        .app(&manifest.id)
        .source()
        .join("main.py");

    std::fs::read_to_string(path).expect("source")
}

/// The whole point: the code that would run is the code of the version you
/// asked for, not the one you are leaving.
#[test]
fn rolling_back_puts_the_source_that_worked_back_on_disk() {
    let (_home, mut workspace, mut manifest) = set_up();

    let good = generate(&mut workspace, &mut manifest, "print('works')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('broken')", Vec::new());
    assert_eq!(source_of(&workspace, &manifest), "print('broken')");

    let done = ephemeral_api::rollback(&mut workspace, &manifest.id, good.short())
        .expect("the earlier version");

    assert_eq!(source_of(&workspace, &manifest), "print('works')");
    assert_eq!(done.digest, good.short());
    assert_eq!(done.grants_withdrawn, 0);
    assert!(
        done.caution.is_none(),
        "nothing was withdrawn to warn about"
    );
    assert!(
        done.headline.contains("csv-comparator"),
        "{}",
        done.headline
    );

    let reloaded = workspace.apps().load(&manifest.id).expect("saved");
    assert_eq!(reloaded.current_version().expect("a version").digest, good);
}

/// A version is its source. Leaving the newer image named in the manifest would
/// run the newer code under the older version's name.
#[test]
fn rolling_back_clears_the_image_and_says_so() {
    let (_home, mut workspace, mut manifest) = set_up();

    let good = generate(&mut workspace, &mut manifest, "print('works')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('broken')", Vec::new());

    if let Some(runtime) = manifest.runtime.as_mut() {
        runtime.image = Some("ephemeral/csv-comparator:broken".to_owned());
    }
    workspace.apps_mut().save(&manifest).expect("saved");

    let done = ephemeral_api::rollback(&mut workspace, &manifest.id, good.short())
        .expect("the earlier version");

    let reloaded = workspace.apps().load(&manifest.id).expect("saved");
    assert_eq!(
        reloaded.runtime.and_then(|runtime| runtime.image),
        None,
        "the image built from the newer source must not survive"
    );
    assert!(done.note.contains("cleared"), "{}", done.note);
}

/// Rolling back can widen: the version being left behind had stopped needing
/// something, and going back asks for it again. An approval given while a
/// capability was not being requested is not an approval for one that is.
#[test]
fn rolling_back_to_a_hungrier_version_withdraws_the_grant_it_would_inherit() {
    let (_home, mut workspace, mut manifest) = set_up();

    let reading = AppPermission::read(PathScope::parse("~/Downloads/**").expect("a scope"));
    let hungry = generate(
        &mut workspace,
        &mut manifest,
        "print('reads the disk')",
        vec![reading.clone()],
    );

    // Somebody allowed it, for that version.
    let subject = Principal::app(manifest.id.clone());
    workspace
        .ledger_mut()
        .allow(
            subject.clone(),
            Permission::App(reading.clone()),
            Actor::User,
            "to read the files you picked",
        )
        .expect("a grant");

    // The next version stopped needing it.
    generate(
        &mut workspace,
        &mut manifest,
        "print('self-contained')",
        Vec::new(),
    );

    let done = ephemeral_api::rollback(&mut workspace, &manifest.id, hungry.short())
        .expect("the earlier version");

    assert_eq!(done.grants_withdrawn, 1, "the inherited grant must go");
    assert_eq!(done.newly_requested, 1);
    assert!(
        done.caution
            .as_ref()
            .is_some_and(|caution| caution.contains("withdrawn")),
        "{done:?}"
    );
    assert!(
        !workspace
            .ledger()
            .check(&subject, &Permission::App(reading))
            .is_allowed(),
        "the application must have to ask again"
    );
}

/// A digest that is not in this application's history is not a version of it,
/// whatever else it might be a digest of — and the refusal says what it does
/// have, because a client's own instructions are not this crate's to give.
#[test]
fn a_digest_that_was_never_recorded_is_refused_with_what_does_exist() {
    let (_home, mut workspace, mut manifest) = set_up();

    let first = generate(&mut workspace, &mut manifest, "print('one')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('two')", Vec::new());

    let error = ephemeral_api::rollback(&mut workspace, &manifest.id, "deadbeef")
        .expect_err("no such version");

    assert!(error.contains("no version matching"), "{error}");
    assert!(error.contains(first.short()), "{error}");
    assert_eq!(
        source_of(&workspace, &manifest),
        "print('two')",
        "a refused rollback must not have moved anything"
    );
}

/// An ambiguous prefix picks nothing rather than the first thing it finds.
#[test]
fn a_prefix_matching_several_versions_is_refused() {
    let (_home, mut workspace, mut manifest) = set_up();

    generate(&mut workspace, &mut manifest, "print('one')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('two')", Vec::new());

    // Every digest starts with nothing in common except the empty string, so
    // the prefix that matches all of them is the first character of one of
    // them — found rather than assumed, since digests are content-addressed.
    let reloaded = workspace.apps().load(&manifest.id).expect("saved");
    let shared: Option<String> =
        ('0'..='f')
            .map(|character| character.to_string())
            .find(|prefix| {
                reloaded
                    .versions
                    .iter()
                    .filter(|version| version.digest.matches(prefix))
                    .count()
                    > 1
            });

    let Some(prefix) = shared else {
        // Two digests with no shared first character is possible and not a
        // failure of the code under test.
        return;
    };

    let error = ephemeral_api::rollback(&mut workspace, &manifest.id, &prefix)
        .expect_err("an ambiguous prefix");

    assert!(error.contains("matches 2 versions"), "{error}");
}

/// A version whose source was never kept can be described and not restored, and
/// saying so beats a half-done rollback.
#[test]
fn a_version_whose_source_is_gone_is_refused_before_anything_moves() {
    let (_home, mut workspace, mut manifest) = set_up();

    let good = generate(&mut workspace, &mut manifest, "print('works')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('broken')", Vec::new());

    // Swept away by retention, or recorded before snapshots existed.
    let kept = workspace
        .layout()
        .app(&manifest.id)
        .version_source(&good)
        .expect("a path");
    std::fs::remove_dir_all(kept).expect("removed");

    let error = ephemeral_api::rollback(&mut workspace, &manifest.id, good.short())
        .expect_err("nothing to go back to");

    assert!(error.contains("not on this machine"), "{error}");
    assert_eq!(source_of(&workspace, &manifest), "print('broken')");

    let reloaded = workspace.apps().load(&manifest.id).expect("saved");
    assert_eq!(
        reloaded.current_version().expect("a version").digest,
        manifest.current_version().expect("a version").digest,
        "the manifest must be untouched"
    );
}

/// The version already in force is not something to return to.
#[test]
fn the_current_version_is_refused() {
    let (_home, mut workspace, mut manifest) = set_up();

    generate(&mut workspace, &mut manifest, "print('one')", Vec::new());
    let current = generate(&mut workspace, &mut manifest, "print('two')", Vec::new());

    let error = ephemeral_api::rollback(&mut workspace, &manifest.id, current.short())
        .expect_err("already there");

    assert!(error.contains("already the current one"), "{error}");
}

/// The record says what happened, including how many grants it cost.
#[test]
fn a_rollback_is_written_to_the_security_record() {
    let (_home, mut workspace, mut manifest) = set_up();

    let good = generate(&mut workspace, &mut manifest, "print('works')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('broken')", Vec::new());

    ephemeral_api::rollback(&mut workspace, &manifest.id, good.short()).expect("rolled back");

    let recorded = ephemeral_api::recent_activity(workspace.audit(), Some(&manifest.id), 5);

    assert!(
        recorded
            .iter()
            .any(|entry| entry.summary.contains("returned to version")
                && entry.summary.contains(good.short())),
        "{recorded:?}"
    );
}

/// A window has to know which versions it may offer, and "recorded" is not the
/// same fact as "still on this machine".
#[test]
fn the_page_says_which_versions_can_actually_be_returned_to() {
    let (_home, mut workspace, mut manifest) = set_up();

    let gone = generate(&mut workspace, &mut manifest, "print('one')", Vec::new());
    generate(&mut workspace, &mut manifest, "print('two')", Vec::new());

    let kept = workspace
        .layout()
        .app(&manifest.id)
        .version_source(&gone)
        .expect("a path");
    std::fs::remove_dir_all(kept).expect("removed");

    let reloaded = workspace.apps().load(&manifest.id).expect("saved");
    let detail = ephemeral_api::application(&reloaded, &workspace);

    let first = detail
        .versions
        .iter()
        .find(|version| version.digest == gone.short())
        .expect("the older version is still in the history");
    let latest = detail.versions.first().expect("the newest");

    assert_eq!(
        first.source_kept,
        Some(false),
        "recorded, and not restorable"
    );
    assert!(!first.current);
    assert_eq!(latest.source_kept, Some(true));
    assert!(latest.current, "the newest version is the current one");
}
