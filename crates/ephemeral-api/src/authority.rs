//! Where the two permission systems actually bite.
//!
//! [ADR-0003](https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md)
//! decided the model: Ephemeral's own authority and an application's are
//! separate, neither inherits from the other, and **Ephemeral's is necessary
//! but not sufficient** — for an application to read a folder, both it and
//! Ephemeral must be permitted, so revoking a meta-permission disables that
//! capability everywhere at once.
//!
//! All of that was modelled and none of it was consulted. `check_app` carried a
//! doc comment calling it "the check enforcement points should use" while every
//! enforcement point used something else: the sandbox was built from an
//! application's grants alone, Docker was driven without asking whether
//! Ephemeral may drive Docker, and a model provider was called without asking
//! whether Ephemeral may reach the network. A permission system nothing consults
//! is a description of a permission system.
//!
//! This module is what consults it. It decides nothing itself — every answer
//! comes from [`PermissionLedger`] — and it exists here, in the layer both
//! clients share, so that a window and a terminal cannot enforce differently.
//!
//! ## Default deny, and what that costs
//!
//! A fresh workspace holds nothing, including for Ephemeral, so the first
//! `ephemeral run` on a new machine is refused until somebody grants Ephemeral
//! the authority to drive a container runtime. That is the model working:
//! forgetting yields no privilege rather than all of it. What it must never be
//! is *mysterious*, so every refusal here names the permission, says who is
//! missing it, and gives the exact command that grants it.

use serde::{Deserialize, Serialize};

use ephemeral_core::{
    AppId, AppManifest,
    permission::{
        AppPermission, EffectiveDecision, MetaPermission, Permission, PermissionLedger, RiskLevel,
    },
};

use crate::operation::Failure;

/// What Ephemeral needs before it can drive a container runtime.
///
/// Building an image, starting a container, reading its logs and tearing it
/// down are all this one permission: they are the same authority used at
/// different moments, and splitting them would ask a person four questions with
/// one answer.
pub const RUNTIME: MetaPermission = MetaPermission::UseDocker;

/// What Ephemeral needs before it can reach a model provider off this machine.
pub const HOSTED_PROVIDER: MetaPermission = MetaPermission::NetworkAccess;

/// What Ephemeral needs before it may use a credential it has been given.
pub const CREDENTIAL: MetaPermission = MetaPermission::AccessCredentials;

/// Whether Ephemeral itself may do something, and what to say if it may not.
///
/// # Errors
///
/// A refusal naming the permission and the command that grants it. The wording
/// is the ledger's own, so a refusal reads the same in a window as in a
/// terminal.
pub fn require(ledger: &PermissionLedger, permission: &MetaPermission) -> Result<(), Failure> {
    if ledger
        .check(
            &ephemeral_core::Principal::Ephemeral,
            &Permission::Meta(permission.clone()),
        )
        .is_allowed()
    {
        return Ok(());
    }

    let remedy = grant_argument(permission).map_or_else(
        || "You can grant it from Ephemeral's own permissions.".to_owned(),
        |written| format!("Grant it with `ephemeral grant ephemeral {written}`."),
    );

    Err(format!(
        "Ephemeral has not been allowed to {}, and nothing that needs it can run until it is. \
         {remedy}",
        permission.describe()
    ))
}

/// How a meta-permission is written on the command line.
///
/// `None` for a capability this version has no word for. [`MetaPermission`] is
/// non-exhaustive, so a newer one can arrive here, and a refusal naming a
/// command that does not work would be worse than one naming no command at
/// all — the test below keeps the gap from opening quietly for the ones that
/// exist today.
#[must_use]
pub fn grant_argument(permission: &MetaPermission) -> Option<String> {
    let written = match permission {
        MetaPermission::FilesystemRead { scope } => format!("read:{}", scope.display_path()),
        MetaPermission::FilesystemWrite { scope } => format!("write:{}", scope.display_path()),
        MetaPermission::ExecuteProcesses => "execute".to_owned(),
        MetaPermission::InstallDependencies => "install-deps".to_owned(),
        MetaPermission::NetworkAccess => "network".to_owned(),
        MetaPermission::UseDocker => "docker".to_owned(),
        MetaPermission::InstallDocker => "docker-install".to_owned(),
        MetaPermission::PullImages => "pull-images".to_owned(),
        MetaPermission::ReadEnvironment => "env".to_owned(),
        MetaPermission::AccessKeychain => "keychain".to_owned(),
        MetaPermission::AccessCredentials => "credentials".to_owned(),
        MetaPermission::CreateShortcuts => "shortcuts".to_owned(),
        MetaPermission::SendNotifications => "notifications".to_owned(),
        MetaPermission::Camera => "camera".to_owned(),
        MetaPermission::Microphone => "microphone".to_owned(),
        MetaPermission::Location => "location".to_owned(),
        MetaPermission::Contacts => "contacts".to_owned(),
        MetaPermission::Calendar => "calendar".to_owned(),
        MetaPermission::BrowserData => "browser-data".to_owned(),
        MetaPermission::ExternalDevices => "devices".to_owned(),
        MetaPermission::SelfUpdate => "self-update".to_owned(),
        _ => return None,
    };

    Some(written)
}

/// One capability an application holds, and whether it can actually be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// What the application was allowed to do.
    pub permission: AppPermission,

    /// Whether it can be used right now.
    ///
    /// `false` when Ephemeral itself lacks the matching authority. The grant is
    /// still there and still the person's decision; it simply does nothing
    /// until Ephemeral is permitted too.
    pub effective: bool,

    /// What Ephemeral is missing, when that is why this is inert.
    pub blocked_by: Option<MetaPermission>,
}

/// What an application may actually do, and what it holds in name only.
///
/// The distinction is the whole point of the two-tier model, and it is the
/// answer every enforcement point needs: the sandbox mounts what is
/// [`Grants::effective`], and a person is shown the rest as inert rather than
/// being told they have nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    /// Every allowed capability, in the order the ledger holds them.
    pub held: Vec<Held>,
}

impl Grants {
    /// What this application has been allowed to do and Ephemeral may carry out.
    ///
    /// This is what a sandbox is built from. Anything Ephemeral is not itself
    /// permitted to do is absent, which is what makes revoking a meta-permission
    /// disable the capability for every application at once rather than
    /// producing a note somewhere.
    #[must_use]
    pub fn effective(&self) -> Vec<AppPermission> {
        self.held
            .iter()
            .filter(|held| held.effective)
            .map(|held| held.permission.clone())
            .collect()
    }

    /// The capabilities that exist on paper and do nothing.
    #[must_use]
    pub fn inert(&self) -> Vec<&Held> {
        self.held.iter().filter(|held| !held.effective).collect()
    }

    /// What to tell somebody whose application is quieter than they expect.
    ///
    /// `None` when everything granted works, because a warning that fires when
    /// nothing is wrong is a warning nobody reads.
    #[must_use]
    pub fn explain_inert(&self) -> Option<String> {
        let inert = self.inert();
        let first = inert.first()?;
        let missing = first.blocked_by.as_ref()?;

        let remedy = grant_argument(missing).map_or_else(
            || "You can grant it from Ephemeral's own permissions.".to_owned(),
            |written| format!("Grant it with `ephemeral grant ephemeral {written}`."),
        );

        Some(format!(
            "{} of its permission(s) do nothing right now, because Ephemeral itself has not been \
             allowed to {}. {remedy}",
            inert.len(),
            missing.describe()
        ))
    }
}

/// What an application holds, judged by both halves of the model.
#[must_use]
pub fn grants(ledger: &PermissionLedger, app: &AppId) -> Grants {
    let subject = ephemeral_core::Principal::app(app.clone());

    let held = ledger
        .active_grants(&subject)
        .into_iter()
        .filter(|grant| grant.decision.is_allowed())
        .filter_map(|grant| match &grant.permission {
            Permission::App(permission) => Some(permission.clone()),
            // Ephemeral's own authority is never an application's, so it cannot
            // reach a sandbox even by accident. The ledger refuses to record
            // one against an application at all; this is the second wall.
            Permission::Meta(_) => None,
        })
        .map(|permission| match ledger.check_app(app, &permission) {
            EffectiveDecision::Allowed => Held {
                permission,
                effective: true,
                blocked_by: None,
            },
            EffectiveDecision::MetaDenied { required } => Held {
                permission,
                effective: false,
                blocked_by: Some(required),
            },
            // Unreachable through this iterator — every permission here came
            // from an active `Allow` for this application — and handled rather
            // than asserted, because an enforcement point that panics on a
            // state it did not expect is worse than one that refuses.
            EffectiveDecision::AppDenied { .. } => Held {
                permission,
                effective: false,
                blocked_by: None,
            },
        })
        .collect();

    Grants { held }
}

/// The highest risk among what an application can actually use.
///
/// Judged on effective capabilities, not granted ones: an application whose
/// only dangerous permission is inert is not currently dangerous, and drawing
/// it as though it were teaches people to ignore the colour.
#[must_use]
pub fn highest_effective_risk(ledger: &PermissionLedger, app: &AppId) -> Option<RiskLevel> {
    grants(ledger, app)
        .effective()
        .iter()
        .map(AppPermission::risk)
        .max()
}

/// Whether this application is currently holding a container.
///
/// Used where a permission change has to reach something already running, since
/// a sandbox is built once, at start: revoking a grant an application is using
/// changes nothing about the container that already has it.
#[must_use]
pub fn is_running(manifest: &AppManifest) -> bool {
    manifest.lifecycle.state().requires_runtime()
        && manifest.lifecycle.state() != ephemeral_core::LifecycleState::Ready
}

#[cfg(test)]
mod tests {
    use super::*;

    use ephemeral_core::{
        Actor, Principal,
        permission::{HostScope, PathScope},
    };

    fn app() -> AppId {
        AppId::parse("csv-comparator").expect("a valid id")
    }

    fn downloads() -> AppPermission {
        AppPermission::read(PathScope::parse("~/Downloads/**").expect("a scope"))
    }

    fn allow(ledger: &mut PermissionLedger, subject: Principal, permission: Permission) {
        ledger
            .allow(subject, permission, Actor::User, "for a test")
            .expect("a person may grant");
    }

    /// The refusal a person meets on a fresh machine. It has to name what is
    /// missing and how to fix it, or default-deny reads as broken software.
    #[test]
    fn a_missing_authority_is_refused_with_the_command_that_grants_it() {
        let ledger = PermissionLedger::new();

        let error = require(&ledger, &RUNTIME).expect_err("nothing is granted yet");

        assert!(error.contains("Ephemeral has not been allowed"), "{error}");
        assert!(
            error.contains("ephemeral grant ephemeral docker"),
            "a refusal with no way forward is a dead end: {error}"
        );
    }

    #[test]
    fn granted_authority_is_permitted() {
        let mut ledger = PermissionLedger::new();
        allow(&mut ledger, Principal::Ephemeral, Permission::Meta(RUNTIME));

        assert!(require(&ledger, &RUNTIME).is_ok());
    }

    /// The rule ADR-0003 states and nothing enforced: Ephemeral's permission is
    /// necessary as well as the application's. A grant whose meta half is
    /// missing is held and inert, not held and working.
    #[test]
    fn a_grant_whose_meta_half_is_missing_does_nothing() {
        let mut ledger = PermissionLedger::new();
        allow(
            &mut ledger,
            Principal::app(app()),
            Permission::App(downloads()),
        );

        let held = grants(&ledger, &app());

        assert_eq!(held.held.len(), 1, "the grant is still the person's");
        assert!(
            held.effective().is_empty(),
            "and it does nothing until Ephemeral may read that folder too"
        );
        assert_eq!(
            held.inert().len(),
            1,
            "which is said out loud rather than silently"
        );

        let explanation = held.explain_inert().expect("an explanation");
        assert!(
            explanation.contains("ephemeral grant ephemeral read:"),
            "{explanation}"
        );
    }

    /// And with both halves, it works.
    #[test]
    fn a_grant_with_both_halves_is_what_a_sandbox_is_built_from() {
        let mut ledger = PermissionLedger::new();
        allow(
            &mut ledger,
            Principal::app(app()),
            Permission::App(downloads()),
        );
        allow(
            &mut ledger,
            Principal::Ephemeral,
            Permission::Meta(downloads().required_meta()),
        );

        let held = grants(&ledger, &app());

        assert_eq!(held.effective(), vec![downloads()]);
        assert!(held.inert().is_empty());
        assert!(held.explain_inert().is_none(), "nothing to warn about");
    }

    /// Revoking Ephemeral's authority disables the capability for every
    /// application at once — the property the whole two-tier model exists for,
    /// checked here at the point that decides what a sandbox gets.
    #[test]
    fn revoking_ephemerals_authority_empties_every_sandbox() {
        let mut ledger = PermissionLedger::new();
        let reading = downloads();

        for name in ["app-a", "app-b"] {
            let id = AppId::parse(name).expect("a valid id");
            allow(
                &mut ledger,
                Principal::app(id),
                Permission::App(reading.clone()),
            );
        }
        allow(
            &mut ledger,
            Principal::Ephemeral,
            Permission::Meta(reading.required_meta()),
        );

        for name in ["app-a", "app-b"] {
            let id = AppId::parse(name).expect("a valid id");
            assert_eq!(grants(&ledger, &id).effective().len(), 1);
        }

        ledger
            .revoke(
                &Principal::Ephemeral,
                &Permission::Meta(reading.required_meta()),
                Actor::User,
            )
            .expect("a person may revoke");

        for name in ["app-a", "app-b"] {
            let id = AppId::parse(name).expect("a valid id");
            assert!(
                grants(&ledger, &id).effective().is_empty(),
                "{name} kept a capability after Ephemeral lost the right to carry it out"
            );
        }
    }

    /// A meta-permission cannot be shown as something an application holds.
    #[test]
    fn ephemerals_own_authority_is_never_an_applications() {
        let mut ledger = PermissionLedger::new();
        allow(
            &mut ledger,
            Principal::Ephemeral,
            Permission::Meta(MetaPermission::UseDocker),
        );

        assert!(grants(&ledger, &app()).held.is_empty());
    }

    /// An application whose only dangerous capability is inert is not currently
    /// dangerous, and painting it as though it were teaches people to ignore
    /// the colour.
    #[test]
    fn risk_is_judged_on_what_can_actually_be_used() {
        let mut ledger = PermissionLedger::new();
        let reaching = AppPermission::outbound(HostScope::parse("*").expect("a host"));

        allow(
            &mut ledger,
            Principal::app(app()),
            Permission::App(reaching.clone()),
        );
        assert_eq!(
            highest_effective_risk(&ledger, &app()),
            None,
            "granted and unusable is not the same as usable"
        );

        allow(
            &mut ledger,
            Principal::Ephemeral,
            Permission::Meta(reaching.required_meta()),
        );
        assert_eq!(
            highest_effective_risk(&ledger, &app()),
            Some(reaching.risk())
        );
    }

    /// Every meta-permission has to be grantable, or a refusal could name a
    /// command that does not work.
    #[test]
    fn every_authority_can_be_written_on_a_command_line() {
        for permission in [
            MetaPermission::read(PathScope::parse("~/**").expect("a scope")),
            MetaPermission::write(PathScope::parse("~/**").expect("a scope")),
            MetaPermission::ExecuteProcesses,
            MetaPermission::InstallDependencies,
            MetaPermission::NetworkAccess,
            MetaPermission::UseDocker,
            MetaPermission::InstallDocker,
            MetaPermission::PullImages,
            MetaPermission::ReadEnvironment,
            MetaPermission::AccessKeychain,
            MetaPermission::AccessCredentials,
            MetaPermission::CreateShortcuts,
            MetaPermission::SendNotifications,
            MetaPermission::Camera,
            MetaPermission::Microphone,
            MetaPermission::Location,
            MetaPermission::Contacts,
            MetaPermission::Calendar,
            MetaPermission::BrowserData,
            MetaPermission::ExternalDevices,
            MetaPermission::SelfUpdate,
        ] {
            let written = grant_argument(&permission)
                .unwrap_or_else(|| panic!("{permission:?} has no way to be granted"));
            assert!(
                !written.is_empty(),
                "{permission:?} has no way to be granted"
            );
        }
    }
}
