#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The security invariants, checked where they are enforced.
//!
//! `ephemeral-core`'s `tests/security.rs` states what Ephemeral promises and
//! checks it against the domain model. That is necessary and it is not enough:
//! a rule the model holds and no enforcement point consults is a rule about a
//! data structure. `check_app` carried a doc comment naming itself "the check
//! enforcement points should use" while every enforcement point used something
//! else, and the sandbox was built from an application's grants alone — so
//! revoking Ephemeral's own authority changed a ledger and nothing else.
//!
//! These go through the service layer both clients call, against a real
//! workspace on disk. Where `security.rs` asks "does the model say no", this
//! asks "does the thing that acts ask the model at all".
//!
//! The mapping from each promise to the code that enforces it is in
//! [docs/security/enforcement.md](https://github.com/JGalego/Ephemeral/blob/main/docs/security/enforcement.md).

use ephemeral_api::authority;
use ephemeral_core::{
    Actor, AppId, AppManifest, Principal,
    manifest::RuntimeSpec,
    permission::{AppPermission, HostScope, MetaPermission, PathScope, Permission},
    storage::{AppStore as _, Workspace},
};

fn app(name: &str) -> AppId {
    AppId::parse(name).expect("a valid id")
}

fn reading() -> AppPermission {
    AppPermission::read(PathScope::parse("~/Downloads/**").expect("a scope"))
}

fn set_up() -> (tempfile::TempDir, Workspace) {
    let home = tempfile::tempdir().expect("a temporary directory");
    let mut workspace = Workspace::open(home.path()).expect("a workspace");

    for name in ["app-a", "app-b"] {
        let mut manifest = AppManifest::requested(app(name), name);
        manifest.runtime = Some(RuntimeSpec::docker_job(
            "python:3.12-slim",
            vec!["python".to_owned()],
        ));
        workspace.apps_mut().create(&manifest).expect("created");
    }

    (home, workspace)
}

fn allow(workspace: &mut Workspace, subject: Principal, permission: Permission) {
    workspace
        .ledger_mut()
        .allow(subject, permission, Actor::User, "for a test")
        .expect("a person may grant");
}

/// The rule ADR-0003 states and nothing consulted: an application permission is
/// necessary and not sufficient. Until Ephemeral is allowed to carry it out, a
/// grant is a decision on record that changes nothing about what runs.
#[test]
fn an_application_permission_alone_reaches_nothing() {
    let (_home, mut workspace) = set_up();
    allow(
        &mut workspace,
        Principal::app(app("app-a")),
        Permission::App(reading()),
    );

    let held = authority::grants(workspace.ledger(), &app("app-a"));

    assert_eq!(held.held.len(), 1, "the decision stands");
    assert!(held.effective().is_empty(), "and carries no authority yet");
    assert!(
        held.explain_inert()
            .is_some_and(|why| why.contains("ephemeral grant ephemeral")),
        "and says what would make it real"
    );
}

/// The property the whole two-tier model exists for, at the point that decides
/// what a sandbox contains: one revocation, every application.
#[test]
fn revoking_ephemerals_authority_disables_every_application_at_once() {
    let (_home, mut workspace) = set_up();

    for name in ["app-a", "app-b"] {
        allow(
            &mut workspace,
            Principal::app(app(name)),
            Permission::App(reading()),
        );
    }
    allow(
        &mut workspace,
        Principal::Ephemeral,
        Permission::Meta(reading().required_meta()),
    );

    for name in ["app-a", "app-b"] {
        assert_eq!(
            authority::grants(workspace.ledger(), &app(name)).effective(),
            vec![reading()],
        );
    }

    workspace
        .ledger_mut()
        .revoke(
            &Principal::Ephemeral,
            &Permission::Meta(reading().required_meta()),
            Actor::User,
        )
        .expect("a person may revoke");

    for name in ["app-a", "app-b"] {
        assert!(
            authority::grants(workspace.ledger(), &app(name))
                .effective()
                .is_empty(),
            "{name} kept a capability Ephemeral is no longer allowed to carry out"
        );
    }
}

/// One application gets nothing from another's grants — asked of the thing that
/// builds sandboxes rather than of the ledger.
#[test]
fn one_applications_grant_reaches_no_other_application() {
    let (_home, mut workspace) = set_up();

    allow(
        &mut workspace,
        Principal::app(app("app-a")),
        Permission::App(reading()),
    );
    allow(
        &mut workspace,
        Principal::Ephemeral,
        Permission::Meta(reading().required_meta()),
    );

    assert_eq!(
        authority::grants(workspace.ledger(), &app("app-a")).effective(),
        vec![reading()]
    );
    assert!(
        authority::grants(workspace.ledger(), &app("app-b"))
            .effective()
            .is_empty()
    );
}

/// Ephemeral's own authority is never an application's. The ledger refuses to
/// record one against an application at all, and what feeds a sandbox drops
/// them as well — two walls, because this is the one that must not fall.
#[test]
fn ephemerals_authority_cannot_become_an_applications() {
    let (_home, mut workspace) = set_up();

    let refused = workspace.ledger_mut().allow(
        Principal::app(app("app-a")),
        Permission::Meta(MetaPermission::UseDocker),
        Actor::User,
        "surely not",
    );
    assert!(refused.is_err(), "the ledger refuses it outright");

    allow(
        &mut workspace,
        Principal::Ephemeral,
        Permission::Meta(MetaPermission::UseDocker),
    );
    assert!(
        authority::grants(workspace.ledger(), &app("app-a"))
            .held
            .is_empty(),
        "and nothing Ephemeral holds appears as something an application holds"
    );
}

/// Default deny, including for Ephemeral. A new installation may do nothing
/// until somebody says so — and is told exactly what to say.
#[test]
fn a_new_installation_grants_ephemeral_nothing() {
    let (_home, workspace) = set_up();

    for permission in [
        authority::RUNTIME,
        authority::HOSTED_PROVIDER,
        authority::CREDENTIAL,
    ] {
        let error = authority::require(workspace.ledger(), &permission)
            .expect_err("nothing is granted on a fresh machine");

        assert!(error.contains("has not been allowed"), "{error}");
        assert!(
            error.contains("ephemeral grant ephemeral"),
            "a refusal with no way forward is a dead end: {error}"
        );
    }
}

/// An explicit denial is not undone by a later grant from anyone but a person,
/// and the enforcement point reads the same ledger that says so.
#[test]
fn a_denial_survives_and_keeps_the_sandbox_empty() {
    let (_home, mut workspace) = set_up();

    allow(
        &mut workspace,
        Principal::Ephemeral,
        Permission::Meta(reading().required_meta()),
    );
    workspace
        .ledger_mut()
        .deny(
            Principal::app(app("app-a")),
            Permission::App(reading()),
            Actor::User,
            "no",
        )
        .expect("a person may deny");

    let refused = workspace.ledger_mut().allow(
        Principal::app(app("app-a")),
        Permission::App(reading()),
        Actor::Agent,
        "the model would like this",
    );

    assert!(refused.is_err(), "an agent may not grant anything");
    assert!(
        authority::grants(workspace.ledger(), &app("app-a"))
            .effective()
            .is_empty(),
        "and the sandbox sees nothing either way"
    );
}

/// Risk is judged on what can be used. An application holding the widest
/// permission Ephemeral offers, which Ephemeral may not carry out, is not
/// currently dangerous — and saying it is teaches people to ignore the word.
#[test]
fn risk_reported_to_a_client_is_the_risk_it_can_actually_reach() {
    let (_home, mut workspace) = set_up();
    let anywhere = AppPermission::outbound(HostScope::parse("*").expect("a host"));

    allow(
        &mut workspace,
        Principal::app(app("app-a")),
        Permission::App(anywhere.clone()),
    );
    assert_eq!(
        authority::highest_effective_risk(workspace.ledger(), &app("app-a")),
        None
    );

    allow(
        &mut workspace,
        Principal::Ephemeral,
        Permission::Meta(anywhere.required_meta()),
    );
    assert_eq!(
        authority::highest_effective_risk(workspace.ledger(), &app("app-a")),
        Some(anywhere.risk())
    );
}

/// What a client draws and what a sandbox contains are the same answer, because
/// they come from the same function. Two clients computing "what does this hold"
/// separately is how a window ends up reassuring somebody about an application
/// the terminal is worried about.
#[test]
fn the_page_and_the_sandbox_agree_about_what_an_application_holds() {
    let (_home, mut workspace) = set_up();

    allow(
        &mut workspace,
        Principal::app(app("app-a")),
        Permission::App(reading()),
    );

    let manifest = workspace.apps().load(&app("app-a")).expect("saved");
    let page = ephemeral_api::application(&manifest, &workspace);
    let sandbox = authority::grants(workspace.ledger(), &app("app-a")).effective();

    assert_eq!(page.permissions.allowed.len(), 1, "held, and shown as held");
    assert!(!page.permissions.allowed[0].effective, "and shown as inert");
    assert!(page.permissions.isolated, "because it reaches nothing");
    assert!(sandbox.is_empty(), "which is what the sandbox does");
    assert_eq!(page.summary.granted, 0, "and what the list counts");
}
