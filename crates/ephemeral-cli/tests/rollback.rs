#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Rolling back, through the same functions the command calls.
//!
//! The domain half and the storage half are each tested in `ephemeral-core`.
//! What is only true when they are used together is what this covers: that the
//! bytes on disk really go back, and that a rollback which asks for more than
//! the version it replaces does not inherit the approval given to that version.

use ephemeral_core::{
    Actor, AppId, Principal,
    manifest::{AppManifest, RuntimeSpec},
    permission::{AppPermission, PathScope, Permission},
    storage::Workspace,
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

/// Generates a version: writes the source and keeps it, the way the real
/// generate command does.
fn generate(
    workspace: &Workspace,
    manifest: &mut AppManifest,
    contents: &str,
    requests: Vec<AppPermission>,
) {
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
}

fn set_up() -> (tempfile::TempDir, Workspace, AppManifest) {
    let home = tempfile::tempdir().expect("a temporary directory");
    let workspace = Workspace::open(home.path()).expect("a workspace");
    let id = AppId::parse("csv-comparator").expect("a valid id");
    workspace.apps().prepare(&id).expect("prepared");

    let mut manifest = AppManifest::requested(id, "CSV comparator");
    manifest.runtime = Some(RuntimeSpec::docker_job(
        "python:3.12-slim",
        vec!["python".to_owned()],
    ));
    (home, workspace, manifest)
}

/// The whole point: the code that would run is the code of the version you
/// asked for, not the one you are leaving.
#[test]
fn rolling_back_puts_the_source_that_worked_back_on_disk() {
    let (_home, workspace, mut manifest) = set_up();

    generate(&workspace, &mut manifest, "print('works')", Vec::new());
    let good = manifest
        .current_version()
        .expect("a version")
        .digest
        .clone();

    generate(&workspace, &mut manifest, "print('broken')", Vec::new());

    let source = workspace
        .layout()
        .app(&manifest.id)
        .source()
        .join("main.py");
    assert_eq!(
        std::fs::read_to_string(&source).expect("source"),
        "print('broken')"
    );

    manifest.revert_to(&good).expect("the earlier version");
    workspace
        .apps()
        .restore_version(&manifest.id, &good)
        .expect("restored");

    assert_eq!(
        std::fs::read_to_string(&source).expect("source"),
        "print('works')"
    );
    assert_eq!(manifest.current_version().expect("a version").digest, good);
}

/// Rolling back can widen: the version being left behind had stopped needing
/// something, and going back asks for it again. An approval given while a
/// capability was not being requested is not an approval for one that is.
#[test]
fn rolling_back_to_a_hungrier_version_withdraws_the_grant_it_would_inherit() {
    let (_home, mut workspace, mut manifest) = set_up();

    let reading = AppPermission::read(PathScope::parse("~/Downloads/**").expect("a scope"));

    generate(
        &workspace,
        &mut manifest,
        "print('reads the disk')",
        vec![reading.clone()],
    );
    let hungry = manifest
        .current_version()
        .expect("a version")
        .digest
        .clone();

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
        &workspace,
        &mut manifest,
        "print('self-contained')",
        Vec::new(),
    );

    let delta = manifest.revert_to(&hungry).expect("the earlier version");
    assert!(delta.widens(), "going back should ask for the disk again");

    let withdrawn = delta
        .added
        .iter()
        .map(|permission| {
            workspace
                .ledger_mut()
                .revoke(&subject, &Permission::App(permission.clone()), Actor::User)
                .unwrap_or(0)
        })
        .sum::<usize>();

    assert_eq!(withdrawn, 1, "the inherited grant must be withdrawn");
    assert!(
        !workspace
            .ledger()
            .check(&subject, &Permission::App(reading))
            .is_allowed(),
        "the application must have to ask again"
    );
}
