//! Security invariants, stated as executable promises.
//!
//! Every test here corresponds to something [`SECURITY.md`] claims about
//! Ephemeral. They are deliberately written from the outside — through the
//! public API, the way a client would use it — so that they check the *product's*
//! promises rather than an implementation detail that happens to hold today.
//!
//! If one of these fails, that is a vulnerability, not a broken test. A change
//! that weakens one should be treated the same way.
//!
//! [`SECURITY.md`]: https://github.com/JGalego/Ephemeral/blob/main/SECURITY.md

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ephemeral_core::{
    Actor, AppId, Principal,
    audit::{AuditEvent, AuditLog},
    lifecycle::{Lifecycle, LifecycleEvent, LifecycleState, TransitionContext, TransitionRequest},
    manifest::{AppManifest, ResourceLimits, RuntimeSpec},
    permission::{
        AppPermission, AppPermissions, Decision, FilesystemRule, HostScope, MetaPermission,
        PathScope, Permission, PermissionError, PermissionLedger, ProcessPolicy,
    },
    storage::{AppStore, MemoryStore, StorageLayout},
};

fn id(value: &str) -> AppId {
    AppId::parse(value).unwrap()
}

fn scope(path: &str) -> PathScope {
    PathScope::parse(path).unwrap()
}

fn host(name: &str) -> HostScope {
    HostScope::parse(name).unwrap()
}

fn manifest(value: &str) -> AppManifest {
    AppManifest::new(
        id(value),
        "Test App",
        RuntimeSpec::docker_job("alpine", vec!["true".to_owned()]),
    )
}

/// A ledger in which Ephemeral holds the broad capabilities it needs on a real
/// machine. Every test starts from here, because the interesting question is
/// what an *application* can do while Ephemeral can do a great deal.
fn ephemeral_with_full_capability() -> PermissionLedger {
    let mut ledger = PermissionLedger::new();
    for permission in [
        MetaPermission::read(scope("~/**")),
        MetaPermission::write(scope("~/**")),
        MetaPermission::ExecuteProcesses,
        MetaPermission::NetworkAccess,
        MetaPermission::UseDocker,
        MetaPermission::AccessCredentials,
        MetaPermission::Camera,
    ] {
        ledger
            .allow(
                Principal::Ephemeral,
                permission,
                Actor::User,
                "granted during setup",
            )
            .unwrap();
    }
    ledger
}

// ---------------------------------------------------------------------------
// "A generated application inherits nothing from Ephemeral."
// ---------------------------------------------------------------------------

/// Ephemeral can read the entire home directory. A generated application, with
/// no grants of its own, can read nothing at all.
#[test]
fn an_application_inherits_none_of_ephemerals_permissions() {
    let ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));

    for wanted in [
        AppPermission::read(scope("~/Downloads/a.csv")),
        AppPermission::read(scope("~/.ssh/id_rsa")),
        AppPermission::write(scope("~/Documents/notes.txt")),
        AppPermission::outbound(host("api.example.com")),
        AppPermission::ExecuteProcesses,
        AppPermission::Camera,
    ] {
        assert_eq!(
            ledger.check(&app, &Permission::App(wanted.clone())),
            Decision::Deny,
            "an application must not inherit {wanted} from Ephemeral"
        );
    }
}

/// Authority does not flow upwards either: granting an app something gives
/// Ephemeral nothing.
#[test]
fn ephemeral_inherits_nothing_from_an_application() {
    let mut ledger = PermissionLedger::new();
    ledger
        .allow(
            Principal::app(id("csv-comparator")),
            AppPermission::Camera,
            Actor::User,
            "it asked",
        )
        .unwrap();

    assert_eq!(
        ledger.check(
            &Principal::Ephemeral,
            &Permission::Meta(MetaPermission::Camera)
        ),
        Decision::Deny
    );
}

/// The two permission spaces cannot be substituted for one another, even by a
/// caller that tries.
#[test]
fn the_two_permission_systems_cannot_be_collapsed_into_one() {
    let mut ledger = PermissionLedger::new();

    let error = ledger
        .allow(
            Principal::app(id("csv-comparator")),
            MetaPermission::UseDocker,
            Actor::User,
            "let it drive Docker",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PermissionError::WrongPermissionSpace { .. }
    ));

    let error = ledger
        .allow(
            Principal::Ephemeral,
            AppPermission::Camera,
            Actor::User,
            "why not",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        PermissionError::WrongPermissionSpace { .. }
    ));
}

// ---------------------------------------------------------------------------
// "App A cannot access app B's files."
// ---------------------------------------------------------------------------

/// One application's grants say nothing about another's, however similarly they
/// are named.
#[test]
fn one_application_gets_nothing_from_anothers_grants() {
    let mut ledger = ephemeral_with_full_capability();
    ledger
        .allow(
            Principal::app(id("app-a")),
            AppPermission::read(scope("~/Documents/**")),
            Actor::User,
            "it asked",
        )
        .unwrap();

    let wanted = Permission::App(AppPermission::read(scope("~/Documents/notes.txt")));

    assert_eq!(
        ledger.check(&Principal::app(id("app-a")), &wanted),
        Decision::Allow
    );
    for other in ["app-b", "app", "app-a-2"] {
        assert_eq!(
            ledger.check(&Principal::app(id(other)), &wanted),
            Decision::Deny,
            "{other} must not benefit from app-a's grant"
        );
    }
}

/// No application's storage tree contains or overlaps another's, including when
/// one identifier is a prefix of the other.
#[test]
fn one_application_cannot_reach_anothers_storage() {
    let layout = StorageLayout::new("/data/ephemeral");
    let a = layout.app(&id("app"));
    let b = layout.app(&id("app-a"));

    assert!(!a.root().starts_with(b.root()));
    assert!(!b.root().starts_with(a.root()));

    // Nor can a path an application declares reach across.
    for hostile in ["../app-a/data", "../../apps/app-a", "..", "/data/ephemeral"] {
        assert_eq!(
            a.resolve(hostile),
            None,
            "{hostile:?} must not resolve out of the application's directory"
        );
    }
}

/// Deleting one application must not disturb another's record or permissions.
#[test]
fn removing_one_application_leaves_the_others_intact() {
    let mut store = MemoryStore::new();
    store.save(&manifest("app-a")).unwrap();
    store.save(&manifest("app-b")).unwrap();

    let mut ledger = ephemeral_with_full_capability();
    for app in ["app-a", "app-b"] {
        ledger
            .allow(
                Principal::app(id(app)),
                AppPermission::Camera,
                Actor::User,
                "it asked",
            )
            .unwrap();
    }

    store.remove(&id("app-a")).unwrap();
    ledger.revoke_all(&Principal::app(id("app-a")));

    assert!(store.load(&id("app-b")).is_ok());
    assert!(
        ledger
            .check_app(&id("app-b"), &AppPermission::Camera)
            .is_allowed()
    );
}

// ---------------------------------------------------------------------------
// "A denied permission is actually denied."
// ---------------------------------------------------------------------------

#[test]
fn a_denial_cannot_be_overridden_by_a_later_grant() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));
    let permission = AppPermission::read(scope("~/Downloads/**"));

    ledger
        .deny(app.clone(), permission.clone(), Actor::User, "no")
        .unwrap();
    ledger
        .allow(app.clone(), permission, Actor::User, "changed my mind?")
        .unwrap();

    assert_eq!(
        ledger.check(
            &app,
            &Permission::App(AppPermission::read(scope("~/Downloads/a.csv")))
        ),
        Decision::Deny,
        "an explicit denial must win regardless of what was recorded afterwards"
    );
}

/// Revoking must actually stop the thing the user asked to stop, even when the
/// grant in place is broader than the revocation.
#[test]
fn revocation_stops_what_the_user_asked_to_stop() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));

    ledger
        .allow(
            app.clone(),
            AppPermission::read(scope("~/**")),
            Actor::User,
            "yes to everything",
        )
        .unwrap();

    ledger
        .revoke(
            &app,
            &Permission::App(AppPermission::read(scope("~/Downloads/**"))),
            Actor::User,
        )
        .unwrap();

    assert_eq!(
        ledger.check(
            &app,
            &Permission::App(AppPermission::read(scope("~/Downloads/a.csv")))
        ),
        Decision::Deny
    );
}

/// A grant is a region, not a prefix. This is the bug class that turns a narrow
/// permission into a broad one without anyone noticing.
#[test]
fn a_grant_does_not_leak_to_similarly_named_neighbours() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));

    ledger
        .allow(
            app.clone(),
            AppPermission::read(scope("~/Downloads/apartments/**")),
            Actor::User,
            "to compare the files you chose",
        )
        .unwrap();

    for forbidden in [
        "~/Downloads/apartments-private/secret.csv",
        "~/Downloads/apartmentsX",
        "~/Downloads",
        "~/.ssh/id_rsa",
        "/etc/passwd",
    ] {
        assert_eq!(
            ledger.check(
                &app,
                &Permission::App(AppPermission::read(scope(forbidden)))
            ),
            Decision::Deny,
            "{forbidden} must not be covered by a grant on ~/Downloads/apartments"
        );
    }
}

/// Network allow-lists respect the dot boundary, so a lookalike domain is not
/// covered by a subdomain wildcard.
#[test]
fn an_egress_allow_list_does_not_cover_lookalike_domains() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("scraper"));

    ledger
        .allow(
            app.clone(),
            AppPermission::outbound(host("*.example.com")),
            Actor::User,
            "to fetch the pages you asked about",
        )
        .unwrap();

    for forbidden in ["example.com.attacker.net", "notexample.com", "attacker.net"] {
        assert_eq!(
            ledger.check(
                &app,
                &Permission::App(AppPermission::outbound(host(forbidden)))
            ),
            Decision::Deny,
            "{forbidden} must not be covered by *.example.com"
        );
    }
}

/// Revoking a meta-permission disables that capability for every application at
/// once, whatever their manifests say.
#[test]
fn revoking_a_meta_permission_disables_every_application() {
    let mut ledger = ephemeral_with_full_capability();
    for app in ["app-a", "app-b"] {
        ledger
            .allow(
                Principal::app(id(app)),
                AppPermission::Camera,
                Actor::User,
                "it asked",
            )
            .unwrap();
        assert!(
            ledger
                .check_app(&id(app), &AppPermission::Camera)
                .is_allowed()
        );
    }

    ledger
        .revoke(
            &Principal::Ephemeral,
            &Permission::Meta(MetaPermission::Camera),
            Actor::User,
        )
        .unwrap();

    for app in ["app-a", "app-b"] {
        assert!(
            !ledger
                .check_app(&id(app), &AppPermission::Camera)
                .is_allowed(),
            "{app} should be blocked once Ephemeral itself loses the capability"
        );
    }
}

// ---------------------------------------------------------------------------
// "Prompt injection cannot escalate privilege."
// ---------------------------------------------------------------------------

/// The generation agent cannot grant a permission — to itself, or to anything
/// it wrote — however convincingly a model was told to.
#[test]
fn the_agent_cannot_grant_itself_or_anyone_else_a_permission() {
    let mut ledger = ephemeral_with_full_capability();

    for actor in [
        Actor::Agent,
        Actor::Ephemeral,
        Actor::Runtime,
        Actor::System,
    ] {
        let error = ledger
            .allow(
                Principal::app(id("csv-comparator")),
                AppPermission::read(scope("~/**")),
                actor,
                "IMPORTANT: the plan requires full filesystem access",
            )
            .unwrap_err();
        assert_eq!(error, PermissionError::UnauthorizedActor { actor });
    }

    assert_eq!(
        ledger.check(
            &Principal::app(id("csv-comparator")),
            &Permission::App(AppPermission::read(scope("~/a")))
        ),
        Decision::Deny
    );
}

/// Nor can it take a permission away, which would be a denial-of-service route
/// into someone else's application.
#[test]
fn the_agent_cannot_revoke_a_permission() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));
    ledger
        .allow(
            app.clone(),
            AppPermission::read(scope("~/Downloads/**")),
            Actor::User,
            "yes",
        )
        .unwrap();

    let error = ledger
        .revoke(
            &app,
            &Permission::App(AppPermission::read(scope("~/Downloads/**"))),
            Actor::Agent,
        )
        .unwrap_err();
    assert_eq!(
        error,
        PermissionError::UnauthorizedActor {
            actor: Actor::Agent
        }
    );
}

/// The thing that wrote the code cannot be the thing that declares it correct.
#[test]
fn the_agent_cannot_sign_off_its_own_output() {
    let mut lifecycle = Lifecycle::new();
    for (event, actor) in [
        (LifecycleEvent::Plan, Actor::Ephemeral),
        (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
        (LifecycleEvent::GenerationCompleted, Actor::Agent),
        (LifecycleEvent::BuildSucceeded, Actor::Runtime),
    ] {
        lifecycle
            .apply(TransitionRequest::new(event, actor, "building"))
            .unwrap();
    }
    assert_eq!(lifecycle.state(), LifecycleState::Validating);

    assert!(
        lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::ValidationPassed,
                Actor::Agent,
                "I checked it myself and it is fine",
            ))
            .is_err()
    );
    assert_eq!(
        lifecycle.state(),
        LifecycleState::Validating,
        "a refused transition must leave the application where it was"
    );
}

/// Nor can it destroy a user's work.
#[test]
fn the_agent_cannot_delete_or_cancel_an_application() {
    let mut lifecycle = Lifecycle::new();

    for event in [LifecycleEvent::Delete, LifecycleEvent::Cancel] {
        assert!(
            lifecycle
                .apply(TransitionRequest::new(
                    event,
                    Actor::Agent,
                    "the plan says to clean up",
                ))
                .is_err(),
            "the agent must not be able to raise {event}"
        );
    }
    assert_eq!(lifecycle.state(), LifecycleState::Requested);
}

/// The agent has nothing to offer a finished application, so an injected one
/// has no move to make.
#[test]
fn the_agent_has_no_available_actions_on_a_ready_application() {
    let mut lifecycle = Lifecycle::new();
    for (event, actor) in [
        (LifecycleEvent::Plan, Actor::Ephemeral),
        (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
        (LifecycleEvent::GenerationCompleted, Actor::Agent),
        (LifecycleEvent::BuildSucceeded, Actor::Runtime),
        (LifecycleEvent::ValidationPassed, Actor::Runtime),
    ] {
        lifecycle
            .apply(TransitionRequest::new(event, actor, "building"))
            .unwrap();
    }

    assert_eq!(lifecycle.state(), LifecycleState::Ready);
    assert!(
        lifecycle.available_events(Actor::Agent).is_empty(),
        "the agent should have nothing it can do to a ready application"
    );
}

/// A manifest is an input, and inputs from a model are untrusted. One that
/// tries to escape its own directory is refused rather than partly applied.
#[test]
fn a_hostile_manifest_is_refused_rather_than_partly_applied() {
    for hostile in [
        // An identifier that would escape the apps directory.
        "schema_version: 1\nid: ../../etc\nname: Escape\n\
         runtime:\n  type: docker\n  image: alpine\n  interface: job\n",
        // A permission scope that would climb out of the granted region.
        "schema_version: 1\nid: escape\nname: Escape\n\
         runtime:\n  type: docker\n  image: alpine\n  interface: job\n\
         permissions:\n  filesystem:\n    - read: ~/Downloads/../../etc/shadow\n",
        // An artifact path pointing at another application.
        "schema_version: 1\nid: escape\nname: Escape\n\
         runtime:\n  type: docker\n  image: alpine\n  interface: job\n\
         artifacts:\n  source: ../other-app/source\n",
        // A schema this build does not understand.
        "schema_version: 99\nid: escape\nname: Escape\n\
         runtime:\n  type: docker\n  image: alpine\n  interface: job\n",
    ] {
        assert!(
            AppManifest::from_yaml(hostile).is_err(),
            "this manifest should have been refused:\n{hostile}"
        );
    }
}

// ---------------------------------------------------------------------------
// "A deleted application loses runtime access."
// ---------------------------------------------------------------------------

/// Deletion withdraws capability immediately. The user's data survives the
/// recovery period; the application's authority does not.
#[test]
fn a_deleted_application_loses_every_permission_at_once() {
    let mut ledger = ephemeral_with_full_capability();
    let app = Principal::app(id("csv-comparator"));

    for permission in [
        AppPermission::read(scope("~/Downloads/**")),
        AppPermission::outbound(host("api.example.com")),
        AppPermission::Camera,
    ] {
        ledger
            .allow(app.clone(), permission, Actor::User, "it asked")
            .unwrap();
    }

    let mut lifecycle = Lifecycle::new();
    lifecycle
        .apply(TransitionRequest::new(
            LifecycleEvent::Delete,
            Actor::User,
            "done with it",
        ))
        .unwrap();
    let revoked = ledger.revoke_all(&app);

    assert_eq!(revoked, 3);
    assert_eq!(lifecycle.state(), LifecycleState::Deleted);
    assert!(
        !lifecycle.state().is_runnable(),
        "a deleted application must not be runnable"
    );
    assert!(
        !lifecycle.state().holds_runtime_resources(),
        "a deleted application must hold no runtime resources"
    );
    assert!(ledger.active_grants(&app).is_empty());
    assert!(
        !ledger
            .check_app(&id("csv-comparator"), &AppPermission::Camera)
            .is_allowed()
    );
}

/// A user must always be able to stop an application, whatever it is doing.
#[test]
fn a_user_can_delete_an_application_in_any_state() {
    for state in LifecycleState::ALL {
        if state == LifecycleState::Deleted {
            continue;
        }
        assert!(
            state
                .next(LifecycleEvent::Delete, &TransitionContext::default())
                .is_ok(),
            "a user must be able to delete an application that is {state}"
        );
    }
}

// ---------------------------------------------------------------------------
// "Secrets never reach an application, a manifest, or a log."
// ---------------------------------------------------------------------------

/// A manifest records the *names* of settings. There is no field to put a value
/// in, so this is structural rather than a rule someone has to remember.
#[test]
fn a_manifest_cannot_carry_a_secret_value() {
    let app = manifest("csv-comparator").with_permissions(AppPermissions {
        environment: vec!["ANTHROPIC_API_KEY".to_owned()],
        ..AppPermissions::none()
    });

    let yaml = app.to_yaml().unwrap();
    assert!(yaml.contains("ANTHROPIC_API_KEY"), "the name is recorded");

    // A manifest that tries to supply a value is refused.
    assert!(
        AppManifest::from_yaml(
            "schema_version: 1\nid: leaky\nname: Leaky\n\
             runtime:\n  type: docker\n  image: alpine\n  interface: job\n\
             permissions:\n  environment:\n    - name: API_KEY\n      value: sk-secret\n",
        )
        .is_err()
    );
}

/// A secret put into a reason string never reaches the audit record, and the
/// entry is still a valid link in the chain afterwards.
#[test]
fn a_secret_cannot_reach_the_audit_record() {
    let mut log = AuditLog::new();
    log.register_secret("sk-ant-a-real-looking-key");

    log.append(
        Actor::Ephemeral,
        AuditEvent::AppCreated {
            app: id("leaky"),
            purpose: "call the API with sk-ant-a-real-looking-key and DB_PASSWORD=hunter2"
                .to_owned(),
        },
    );

    let serialised = serde_json::to_string(&log).unwrap();
    for secret in ["sk-ant-a-real-looking-key", "hunter2"] {
        assert!(
            !serialised.contains(secret),
            "{secret} reached the audit record: {serialised}"
        );
    }
    log.verify().unwrap();
}

/// The audit log records that a secret was used, and by whom, without recording
/// what it was.
#[test]
fn secret_use_is_recorded_but_secret_values_are_not() {
    let mut log = AuditLog::new();
    log.append(
        Actor::Runtime,
        AuditEvent::SecretAccessed {
            principal: Principal::app(id("csv-comparator")),
            name: "ANTHROPIC_API_KEY".to_owned(),
        },
    );

    let entry = &log.entries()[0];
    assert!(entry.explain().contains("ANTHROPIC_API_KEY"));
    assert!(entry.explain().contains("csv-comparator"));

    let serialised = serde_json::to_string(entry).unwrap();
    assert!(
        !serialised.contains("\"value\""),
        "there must be no field for a secret value: {serialised}"
    );
}

// ---------------------------------------------------------------------------
// "The audit record cannot be quietly altered."
// ---------------------------------------------------------------------------

/// Flipping a recorded decision after the fact is exactly what the chain exists
/// to make visible.
#[test]
fn altering_a_recorded_decision_is_detected() {
    let mut log = AuditLog::new();
    log.append(
        Actor::User,
        AuditEvent::PermissionDecided {
            principal: Principal::app(id("csv-comparator")),
            permission: Permission::App(AppPermission::read(scope("~/Downloads/**"))),
            decision: Decision::Deny,
        },
    );
    log.verify().unwrap();

    // Rewrite the stored record as though the user had allowed it.
    let mut tampered: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&log).unwrap()).unwrap();
    tampered["entries"][0]["event"]["decision"] = serde_json::json!("allow");

    let restored: AuditLog = serde_json::from_value(tampered).unwrap();
    assert!(
        restored.verify().is_err(),
        "a rewritten decision must not verify"
    );
}

// ---------------------------------------------------------------------------
// "Nothing runs unbounded."
// ---------------------------------------------------------------------------

/// Every default is a real ceiling, so the safe configuration is the one people
/// get without asking.
#[test]
fn a_new_application_is_bounded_on_every_axis() {
    let app = manifest("csv-comparator");

    assert!(app.resources.is_valid());
    assert!(app.resources.cpu_millis > 0);
    assert!(app.resources.memory_mib > 0);
    assert!(app.resources.storage_mib > 0);
    assert!(
        app.resources.max_processes > 0,
        "a fork bomb must be bounded"
    );
    assert!(app.resources.max_runtime.is_some());

    assert!(app.budget.is_valid());
    assert!(app.budget.max_repairs > 0);
    assert!(app.budget.max_spend_cents.is_some());
}

/// A zero limit is refused rather than silently meaning "unlimited", which is
/// how ceilings get removed by accident.
#[test]
fn a_zero_limit_is_refused_rather_than_treated_as_unlimited() {
    let broken = AppManifest {
        resources: ResourceLimits {
            memory_mib: 0,
            ..ResourceLimits::default()
        },
        ..manifest("csv-comparator")
    };

    assert!(broken.validate().is_err());

    let mut store = MemoryStore::new();
    assert!(
        store.save(&broken).is_err(),
        "an unbounded application must not reach storage"
    );
}

/// The autonomous repair loop terminates by construction: the budget is spent
/// and the machine refuses to continue rather than looping.
#[test]
fn the_repair_loop_cannot_run_forever() {
    let mut lifecycle = Lifecycle::with_repair_budget(2);
    for (event, actor) in [
        (LifecycleEvent::Plan, Actor::Ephemeral),
        (LifecycleEvent::PlanCompleted, Actor::Ephemeral),
        (LifecycleEvent::GenerationCompleted, Actor::Agent),
    ] {
        lifecycle
            .apply(TransitionRequest::new(event, actor, "building"))
            .unwrap();
    }

    let mut repairs = 0;
    loop {
        lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::BuildSucceeded,
                Actor::Runtime,
                "built",
            ))
            .unwrap();
        lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::ValidationFailed,
                Actor::Runtime,
                "the output was wrong",
            ))
            .unwrap();

        if lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::Repair,
                Actor::Ephemeral,
                "trying again",
            ))
            .is_err()
        {
            break;
        }
        repairs += 1;
        assert!(repairs <= 2, "the repair loop did not terminate");

        lifecycle
            .apply(TransitionRequest::new(
                LifecycleEvent::RepairCompleted,
                Actor::Agent,
                "fixed, maybe",
            ))
            .unwrap();
    }

    assert_eq!(repairs, 2);
    assert_eq!(lifecycle.state(), LifecycleState::ValidationFailed);
}

/// An application that asks for a dangerous capability is flagged, so an
/// interface cannot present it as a routine choice.
#[test]
fn dangerous_requests_are_flagged_for_explicit_confirmation() {
    let dangerous = AppPermissions {
        filesystem: vec![FilesystemRule::Read(scope("~/**"))],
        process: ProcessPolicy { execute: true },
        ..AppPermissions::none()
    };

    let risk = dangerous.highest_risk().unwrap();
    assert!(
        risk.requires_explicit_confirmation(),
        "{risk} should demand an explicit decision"
    );
}

// ---------------------------------------------------------------------------
// "Corrupted state is refused, not interpreted."
// ---------------------------------------------------------------------------

/// A tampered resume state must not be able to return an application to a
/// running state it never legitimately reached.
#[test]
fn a_tampered_lifecycle_cannot_resume_into_a_running_state() {
    let context = TransitionContext {
        resume_state: Some(LifecycleState::Running),
        ..TransitionContext::default()
    };

    assert!(
        LifecycleState::PermissionRequired
            .next(LifecycleEvent::PermissionGranted, &context)
            .is_err(),
        "granting a permission must not be a route into Running"
    );
}

/// A manifest found under the wrong identity is refused, since acting on it
/// would apply one application's permissions under another's name.
#[test]
fn a_manifest_cannot_be_used_under_a_different_identity() {
    let mut store = MemoryStore::new();
    let app = manifest("app-a");
    store.save(&app).unwrap();

    // The in-memory store is keyed by id, so the equivalent attack is simply
    // absent; the filesystem store checks it explicitly and has its own test.
    assert!(store.load(&id("app-b")).is_err());
    assert_eq!(store.load(&id("app-a")).unwrap().id, id("app-a"));
}
