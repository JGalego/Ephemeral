//! The ledger: where permission decisions are recorded and answered.
//!
//! Everything the permission model promises is enforced in
//! [`PermissionLedger::decide`], which is short on purpose. A permission check
//! that needs a diagram to review is a permission check nobody reviews.
//!
//! The rules, in the order they apply:
//!
//! 1. **Only the subject's own grants are considered.** No principal inherits
//!    another's, in either direction. An application gets nothing from
//!    Ephemeral.
//! 2. **Revoked and expired grants are ignored.**
//! 3. **An explicit `Deny` wins**, whenever it was recorded and whatever else
//!    exists.
//! 4. **An `Allow` must cover the request**, by scope containment rather than
//!    equality.
//! 5. **Anything else is `Deny`.** Absence is refusal.
//!
//! Granting is separately constrained: only [`Actor::User`] may decide, and a
//! permission may only be granted to a principal eligible to hold it.

use serde::{Deserialize, Serialize};

use super::{AppPermission, Decision, Grant, MetaPermission, Permission};
use crate::{
    Timestamp,
    actor::Actor,
    identity::{AppId, Principal},
    now,
};

/// Why a permission operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PermissionError {
    /// Something other than a person tried to make a permission decision.
    ///
    /// The structural defence against an autonomous agent — or an injected one —
    /// authorising its own access.
    #[error("{actor} may not decide permissions; only a person can")]
    UnauthorizedActor {
        /// Who tried.
        actor: Actor,
    },

    /// A permission was offered to a principal that cannot hold it.
    ///
    /// Granting a meta-permission to an application, or an application
    /// permission to Ephemeral, is a programming error rather than a policy
    /// question — and exactly the mistake that would collapse the two
    /// permission systems into one.
    #[error("{principal} cannot hold {permission}: it belongs to the other permission system")]
    WrongPermissionSpace {
        /// Who it was offered to.
        principal: Principal,
        /// What was offered.
        permission: Permission,
    },
}

/// Why an application capability was refused.
///
/// Richer than a bare [`Decision`] because "your app is allowed but Ephemeral
/// is not" needs a different fix from "your app is not allowed", and a user
/// deserves to be told which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveDecision {
    /// Both the application and Ephemeral are permitted.
    Allowed,

    /// The application itself has not been granted this.
    AppDenied {
        /// What was asked for.
        permission: AppPermission,
    },

    /// The application is permitted, but Ephemeral is not, so nothing can do
    /// this until the meta-permission is granted.
    MetaDenied {
        /// What Ephemeral needs first.
        required: MetaPermission,
    },
}

impl EffectiveDecision {
    /// Whether the operation may proceed.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// A plain-language explanation of a refusal.
    #[must_use]
    pub fn explain(&self) -> String {
        match self {
            Self::Allowed => "Allowed.".to_owned(),
            Self::AppDenied { permission } => format!(
                "This app has not been allowed to {}. You can allow it from the app's page.",
                permission.describe()
            ),
            Self::MetaDenied { required } => format!(
                "Ephemeral itself has not been allowed to {}, so no app can do this yet. \
                 You can change that in Ephemeral's settings.",
                required.describe()
            ),
        }
    }
}

/// The record of every permission decision that has been made.
///
/// Append-mostly: grants are added and revoked, never edited or removed, so the
/// history of what was allowed when stays answerable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionLedger {
    grants: Vec<Grant>,
}

impl PermissionLedger {
    /// An empty ledger, in which nothing is permitted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a decision.
    ///
    /// # Errors
    ///
    /// - [`PermissionError::UnauthorizedActor`] if `actor` is not
    ///   [`Actor::User`]. Permission decisions are a person's alone; the
    ///   generation agent cannot make one whatever it was told to output.
    /// - [`PermissionError::WrongPermissionSpace`] if the principal cannot hold
    ///   this kind of permission.
    pub fn decide(
        &mut self,
        subject: Principal,
        permission: impl Into<Permission>,
        decision: Decision,
        actor: Actor,
        reason: impl Into<String>,
    ) -> Result<&Grant, PermissionError> {
        if !actor.is_human() {
            return Err(PermissionError::UnauthorizedActor { actor });
        }

        let permission = permission.into();
        if !permission.is_valid_for(&subject) {
            return Err(PermissionError::WrongPermissionSpace {
                principal: subject,
                permission,
            });
        }

        self.grants
            .push(Grant::new(subject, permission, decision, actor, reason));

        Ok(self
            .grants
            .last()
            .unwrap_or_else(|| unreachable!("a grant was just recorded")))
    }

    /// Records an allow. A convenience over [`PermissionLedger::decide`].
    ///
    /// # Errors
    ///
    /// As [`PermissionLedger::decide`].
    pub fn allow(
        &mut self,
        subject: Principal,
        permission: impl Into<Permission>,
        actor: Actor,
        reason: impl Into<String>,
    ) -> Result<&Grant, PermissionError> {
        self.decide(subject, permission, Decision::Allow, actor, reason)
    }

    /// Records a denial. A convenience over [`PermissionLedger::decide`].
    ///
    /// # Errors
    ///
    /// As [`PermissionLedger::decide`].
    pub fn deny(
        &mut self,
        subject: Principal,
        permission: impl Into<Permission>,
        actor: Actor,
        reason: impl Into<String>,
    ) -> Result<&Grant, PermissionError> {
        self.decide(subject, permission, Decision::Deny, actor, reason)
    }

    /// Answers a permission question as of now.
    ///
    /// Default-deny: an absent grant is a refusal.
    #[must_use]
    pub fn check(&self, subject: &Principal, permission: &Permission) -> Decision {
        self.check_at(subject, permission, now())
    }

    /// Answers a permission question as of a given moment.
    ///
    /// Taking the time explicitly keeps expiry testable without waiting.
    #[must_use]
    pub fn check_at(
        &self,
        subject: &Principal,
        permission: &Permission,
        at: Timestamp,
    ) -> Decision {
        let applicable = self.grants.iter().filter(|grant| {
            // Rule 1: only this principal's own grants. This single comparison
            // is what makes the two permission systems separate in practice.
            &grant.subject == subject
                // Rule 2: revoked and expired grants do not count.
                && grant.is_active_at(at)
                // Rule 4: the grant must actually cover what was asked.
                && grant.permission.satisfies(permission)
        });

        let mut allowed = false;
        for grant in applicable {
            match grant.decision {
                // Rule 3: an explicit denial wins, whenever it was recorded.
                Decision::Deny => return Decision::Deny,
                Decision::Allow => allowed = true,
            }
        }

        // Rule 5: absence is refusal.
        if allowed {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }

    /// Answers whether an application may actually do something.
    ///
    /// This is the check enforcement points should use, because it applies both
    /// halves of the model: the application must be permitted **and** Ephemeral
    /// must hold the corresponding meta-permission. Revoking a meta-permission
    /// therefore disables that capability for every application at once.
    #[must_use]
    pub fn check_app(&self, app: &AppId, permission: &AppPermission) -> EffectiveDecision {
        let subject = Principal::app(app.clone());

        if !self
            .check(&subject, &Permission::App(permission.clone()))
            .is_allowed()
        {
            return EffectiveDecision::AppDenied {
                permission: permission.clone(),
            };
        }

        let required = permission.required_meta();
        if !self
            .check(&Principal::Ephemeral, &Permission::Meta(required.clone()))
            .is_allowed()
        {
            return EffectiveDecision::MetaDenied { required };
        }

        EffectiveDecision::Allowed
    }

    /// Revokes every active grant of a permission from a principal.
    ///
    /// Returns how many grants were revoked. Revocation marks grants rather than
    /// deleting them, so the history stays intact.
    ///
    /// **Revocation errs broad, deliberately.** A grant is revoked if it covers
    /// the named permission *or* is covered by it:
    ///
    /// - revoking `~/Downloads/**` withdraws a wider `~/**` grant, because
    ///   leaving something in place that still permits what the user asked to
    ///   stop is the dangerous failure;
    /// - revoking `~/**` withdraws the narrower grants inside it, so "stop
    ///   reading my home directory" does not leave a surviving sub-grant.
    ///
    /// Scopes cannot be partially subtracted, so the alternative to over-
    /// revoking is under-revoking, and only one of those fails safe. A user who
    /// wants the narrower access back grants it again.
    ///
    /// # Errors
    ///
    /// [`PermissionError::UnauthorizedActor`] if `actor` is not a person.
    pub fn revoke(
        &mut self,
        subject: &Principal,
        permission: &Permission,
        actor: Actor,
    ) -> Result<usize, PermissionError> {
        if !actor.is_human() {
            return Err(PermissionError::UnauthorizedActor { actor });
        }

        let at = now();
        let mut revoked = 0;
        for grant in &mut self.grants {
            let overlaps =
                grant.permission.satisfies(permission) || permission.satisfies(&grant.permission);
            if &grant.subject == subject && grant.is_active_at(at) && overlaps {
                grant.revoked_at = Some(at);
                revoked += 1;
            }
        }
        Ok(revoked)
    }

    /// Revokes everything a principal holds.
    ///
    /// Used when an application is deleted: capability is withdrawn immediately,
    /// even though the app's data survives the recovery period ([ADR-0009]).
    /// Unlike [`PermissionLedger::revoke`] this is not restricted to a person,
    /// because taking authority away is always safe — the dangerous direction is
    /// granting it.
    ///
    /// [ADR-0009]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0009-storage-layout-and-retention.md
    pub fn revoke_all(&mut self, subject: &Principal) -> usize {
        let at = now();
        let mut revoked = 0;
        for grant in &mut self.grants {
            if &grant.subject == subject && grant.is_active_at(at) {
                grant.revoked_at = Some(at);
                revoked += 1;
            }
        }
        revoked
    }

    /// Every grant ever recorded, oldest first, including revoked ones.
    #[must_use]
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }

    /// The grants that currently apply to a principal.
    ///
    /// This is what an app's detail page shows under "what this app can do".
    #[must_use]
    pub fn active_grants(&self, subject: &Principal) -> Vec<&Grant> {
        let at = now();
        self.grants
            .iter()
            .filter(|grant| &grant.subject == subject && grant.is_active_at(at))
            .collect()
    }

    /// Every principal that holds or has ever held a grant.
    #[must_use]
    pub fn principals(&self) -> Vec<Principal> {
        let mut principals: Vec<_> = self.grants.iter().map(|g| g.subject.clone()).collect();
        principals.sort();
        principals.dedup();
        principals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{HostScope, PathScope};
    use chrono::Duration;

    fn app_id(id: &str) -> AppId {
        AppId::parse(id).unwrap()
    }

    fn app(id: &str) -> Principal {
        Principal::app(app_id(id))
    }

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    fn host(name: &str) -> HostScope {
        HostScope::parse(name).unwrap()
    }

    fn read(path: &str) -> Permission {
        Permission::App(AppPermission::read(scope(path)))
    }

    /// A ledger in which Ephemeral holds broad meta-permissions, as it would on
    /// a real machine.
    fn ledger_with_capable_ephemeral() -> PermissionLedger {
        let mut ledger = PermissionLedger::new();
        for permission in [
            MetaPermission::read(scope("~/**")),
            MetaPermission::write(scope("~/**")),
            MetaPermission::UseDocker,
            MetaPermission::NetworkAccess,
            MetaPermission::Camera,
        ] {
            ledger
                .allow(Principal::Ephemeral, permission, Actor::User, "setup")
                .unwrap();
        }
        ledger
    }

    // --- default deny --------------------------------------------------------

    #[test]
    fn an_empty_ledger_permits_nothing() {
        let ledger = PermissionLedger::new();

        assert_eq!(
            ledger.check(&app("csv-comparator"), &read("~/Downloads/a.csv")),
            Decision::Deny
        );
        assert_eq!(
            ledger.check(
                &Principal::Ephemeral,
                &Permission::Meta(MetaPermission::UseDocker)
            ),
            Decision::Deny
        );
    }

    #[test]
    fn an_allow_covers_what_it_contains_and_nothing_more() {
        let mut ledger = ledger_with_capable_ephemeral();
        ledger
            .allow(
                app("csv-comparator"),
                AppPermission::read(scope("~/Downloads/apartments/**")),
                Actor::User,
                "to compare the files you chose",
            )
            .unwrap();

        let subject = app("csv-comparator");
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/apartments/a.csv")),
            Decision::Allow
        );
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/taxes/a.csv")),
            Decision::Deny
        );
        assert_eq!(
            ledger.check(&subject, &read("~/.ssh/id_rsa")),
            Decision::Deny
        );
    }

    // --- no inheritance: the central invariant --------------------------------

    /// Ephemeral holding read access to the whole home directory must grant a
    /// generated application nothing whatsoever.
    #[test]
    fn an_application_inherits_nothing_from_ephemeral() {
        let ledger = ledger_with_capable_ephemeral();

        assert_eq!(
            ledger.check(
                &Principal::Ephemeral,
                &Permission::Meta(MetaPermission::read(scope("~/Downloads/a.csv")))
            ),
            Decision::Allow,
            "Ephemeral itself should be permitted"
        );
        assert_eq!(
            ledger.check(&app("csv-comparator"), &read("~/Downloads/a.csv")),
            Decision::Deny,
            "an application must inherit nothing from Ephemeral's grants"
        );
    }

    /// Nor does authority flow the other way: granting an app something does not
    /// give Ephemeral anything.
    #[test]
    fn ephemeral_inherits_nothing_from_an_application() {
        let mut ledger = PermissionLedger::new();
        ledger
            .allow(
                app("csv-comparator"),
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

    /// One application's grants say nothing about another's. This is the
    /// isolation promise between generated apps.
    #[test]
    fn applications_are_isolated_from_each_other() {
        let mut ledger = ledger_with_capable_ephemeral();
        ledger
            .allow(
                app("app-a"),
                AppPermission::read(scope("~/Documents/**")),
                Actor::User,
                "it asked",
            )
            .unwrap();

        assert_eq!(
            ledger.check(&app("app-a"), &read("~/Documents/notes.txt")),
            Decision::Allow
        );
        assert_eq!(
            ledger.check(&app("app-b"), &read("~/Documents/notes.txt")),
            Decision::Deny,
            "app B must not benefit from app A's grant"
        );
    }

    // --- explicit denial wins -------------------------------------------------

    #[test]
    fn an_explicit_denial_beats_an_allow_recorded_before_or_after_it() {
        for deny_first in [true, false] {
            let mut ledger = ledger_with_capable_ephemeral();
            let subject = app("csv-comparator");
            let permission = AppPermission::read(scope("~/Downloads/**"));

            if deny_first {
                ledger
                    .deny(subject.clone(), permission.clone(), Actor::User, "no")
                    .unwrap();
                ledger
                    .allow(subject.clone(), permission.clone(), Actor::User, "yes")
                    .unwrap();
            } else {
                ledger
                    .allow(subject.clone(), permission.clone(), Actor::User, "yes")
                    .unwrap();
                ledger
                    .deny(subject.clone(), permission.clone(), Actor::User, "no")
                    .unwrap();
            }

            assert_eq!(
                ledger.check(&subject, &read("~/Downloads/a.csv")),
                Decision::Deny,
                "a denial must win regardless of order (deny_first={deny_first})"
            );
        }
    }

    /// A denial of a broad scope covers everything inside it, so "never let this
    /// app touch my home directory" means what it says.
    #[test]
    fn a_broad_denial_covers_narrow_requests() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");

        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/Downloads/apartments/**")),
                Actor::User,
                "yes to this folder",
            )
            .unwrap();
        ledger
            .deny(
                subject.clone(),
                AppPermission::read(scope("~/**")),
                Actor::User,
                "actually, nothing in my home directory",
            )
            .unwrap();

        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/apartments/a.csv")),
            Decision::Deny
        );
    }

    // --- only a person decides ------------------------------------------------

    /// The structural anti-injection property: no autonomous component can
    /// authorise anything, whatever a model produced.
    #[test]
    fn only_a_person_can_decide_a_permission() {
        let mut ledger = PermissionLedger::new();

        for actor in [
            Actor::Agent,
            Actor::Ephemeral,
            Actor::Runtime,
            Actor::System,
        ] {
            let error = ledger
                .allow(
                    app("csv-comparator"),
                    AppPermission::read(scope("~/**")),
                    actor,
                    "the plan says this is needed",
                )
                .unwrap_err();
            assert_eq!(error, PermissionError::UnauthorizedActor { actor });
        }

        assert!(
            ledger.grants().is_empty(),
            "a refused decision must not be recorded"
        );
        assert!(
            ledger
                .allow(
                    app("csv-comparator"),
                    AppPermission::read(scope("~/**")),
                    Actor::User,
                    "fine"
                )
                .is_ok()
        );
    }

    #[test]
    fn the_agent_cannot_revoke_either() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");
        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/Downloads/**")),
                Actor::User,
                "yes",
            )
            .unwrap();

        let error = ledger
            .revoke(&subject, &read("~/Downloads/**"), Actor::Agent)
            .unwrap_err();
        assert_eq!(
            error,
            PermissionError::UnauthorizedActor {
                actor: Actor::Agent
            }
        );
    }

    // --- the two spaces stay separate ----------------------------------------

    #[test]
    fn a_meta_permission_cannot_be_granted_to_an_application() {
        let mut ledger = PermissionLedger::new();

        let error = ledger
            .allow(
                app("csv-comparator"),
                MetaPermission::UseDocker,
                Actor::User,
                "sure",
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
                "sure",
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PermissionError::WrongPermissionSpace { .. }
        ));
    }

    // --- the ceiling rule -----------------------------------------------------

    #[test]
    fn an_app_capability_needs_both_halves() {
        let mut ledger = PermissionLedger::new();
        let id = app_id("csv-comparator");
        let wants = AppPermission::read(scope("~/Downloads/apartments/a.csv"));

        // Neither half granted.
        assert!(matches!(
            ledger.check_app(&id, &wants),
            EffectiveDecision::AppDenied { .. }
        ));

        // The app is allowed, but Ephemeral is not.
        ledger
            .allow(
                app("csv-comparator"),
                AppPermission::read(scope("~/Downloads/apartments/**")),
                Actor::User,
                "to compare your files",
            )
            .unwrap();
        let decision = ledger.check_app(&id, &wants);
        assert!(matches!(decision, EffectiveDecision::MetaDenied { .. }));
        assert!(decision.explain().contains("Ephemeral itself"));

        // Both halves granted.
        ledger
            .allow(
                Principal::Ephemeral,
                MetaPermission::read(scope("~/**")),
                Actor::User,
                "setup",
            )
            .unwrap();
        assert!(ledger.check_app(&id, &wants).is_allowed());
    }

    /// Revoking a meta-permission must disable that capability for every
    /// application at once, without touching their manifests.
    #[test]
    fn revoking_a_meta_permission_disables_every_app() {
        let mut ledger = ledger_with_capable_ephemeral();
        for id in ["app-a", "app-b"] {
            ledger
                .allow(app(id), AppPermission::Camera, Actor::User, "it asked")
                .unwrap();
        }

        assert!(
            ledger
                .check_app(&app_id("app-a"), &AppPermission::Camera)
                .is_allowed()
        );
        assert!(
            ledger
                .check_app(&app_id("app-b"), &AppPermission::Camera)
                .is_allowed()
        );

        ledger
            .revoke(
                &Principal::Ephemeral,
                &Permission::Meta(MetaPermission::Camera),
                Actor::User,
            )
            .unwrap();

        for id in ["app-a", "app-b"] {
            let decision = ledger.check_app(&app_id(id), &AppPermission::Camera);
            assert!(
                matches!(decision, EffectiveDecision::MetaDenied { .. }),
                "{id} should be blocked by the missing meta-permission"
            );
        }
    }

    // --- revocation and expiry ------------------------------------------------

    #[test]
    fn revocation_takes_effect_immediately_and_keeps_the_history() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");
        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/Downloads/**")),
                Actor::User,
                "yes",
            )
            .unwrap();
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/a.csv")),
            Decision::Allow
        );

        let revoked = ledger
            .revoke(&subject, &read("~/Downloads/**"), Actor::User)
            .unwrap();

        assert_eq!(revoked, 1);
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/a.csv")),
            Decision::Deny
        );
        assert!(
            ledger.grants().iter().any(Grant::is_revoked),
            "the revoked grant must remain in the record"
        );
        assert!(ledger.active_grants(&subject).is_empty());
    }

    /// Revoking a broad grant revokes the narrow grants it covers, so a user who
    /// says "stop reading my home directory" is not left with a surviving
    /// sub-grant.
    #[test]
    fn revoking_a_broad_scope_revokes_the_narrow_grants_inside_it() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");
        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/Downloads/apartments/**")),
                Actor::User,
                "yes",
            )
            .unwrap();
        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/Documents/**")),
                Actor::User,
                "yes",
            )
            .unwrap();

        let revoked = ledger
            .revoke(
                &subject,
                &Permission::App(AppPermission::read(scope("~/**"))),
                Actor::User,
            )
            .unwrap();

        assert_eq!(revoked, 2);
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/apartments/a.csv")),
            Decision::Deny
        );
    }

    /// The other direction, and the more important one: revoking a narrow scope
    /// must withdraw a wider grant that would still permit it. Under-revoking
    /// would leave the user believing they had stopped something they had not.
    #[test]
    fn revoking_a_narrow_scope_withdraws_the_wider_grant_covering_it() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");
        ledger
            .allow(
                subject.clone(),
                AppPermission::read(scope("~/**")),
                Actor::User,
                "yes to everything",
            )
            .unwrap();

        let revoked = ledger
            .revoke(&subject, &read("~/Downloads/**"), Actor::User)
            .unwrap();

        assert_eq!(revoked, 1);
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/a.csv")),
            Decision::Deny,
            "the thing the user asked to stop must actually stop"
        );
        assert_eq!(
            ledger.check(&subject, &read("~/Documents/a.csv")),
            Decision::Deny,
            "scopes cannot be partially subtracted, so revocation errs broad"
        );
    }

    /// Deleting an application must withdraw its capability at once, even though
    /// its data survives the recovery period.
    #[test]
    fn revoking_everything_leaves_an_application_with_nothing() {
        let mut ledger = ledger_with_capable_ephemeral();
        let subject = app("csv-comparator");
        for permission in [
            AppPermission::read(scope("~/Downloads/**")),
            AppPermission::outbound(host("api.example.com")),
            AppPermission::Camera,
        ] {
            ledger
                .allow(subject.clone(), permission, Actor::User, "it asked")
                .unwrap();
        }

        assert_eq!(ledger.revoke_all(&subject), 3);
        assert!(ledger.active_grants(&subject).is_empty());
        assert_eq!(
            ledger.check(&subject, &read("~/Downloads/a.csv")),
            Decision::Deny
        );
        assert!(
            !ledger
                .check_app(&app_id("csv-comparator"), &AppPermission::Camera)
                .is_allowed()
        );
    }

    #[test]
    fn revoking_one_principal_does_not_touch_another() {
        let mut ledger = ledger_with_capable_ephemeral();
        for id in ["app-a", "app-b"] {
            ledger
                .allow(app(id), AppPermission::Camera, Actor::User, "it asked")
                .unwrap();
        }

        ledger.revoke_all(&app("app-a"));

        assert!(ledger.active_grants(&app("app-a")).is_empty());
        assert_eq!(ledger.active_grants(&app("app-b")).len(), 1);
    }

    #[test]
    fn an_expired_grant_stops_applying_on_its_own() {
        let mut ledger = PermissionLedger::new();
        let subject = app("csv-comparator");
        let expiry = now() + Duration::hours(1);

        let grant = Grant::new(
            subject.clone(),
            AppPermission::read(scope("~/Downloads/**")),
            Decision::Allow,
            Actor::User,
            "for this session",
        )
        .expiring_at(expiry);
        ledger.grants.push(grant);

        assert_eq!(
            ledger.check_at(&subject, &read("~/Downloads/a.csv"), now()),
            Decision::Allow
        );
        assert_eq!(
            ledger.check_at(
                &subject,
                &read("~/Downloads/a.csv"),
                expiry + Duration::seconds(1)
            ),
            Decision::Deny
        );
    }

    // --- inspection and persistence ------------------------------------------

    #[test]
    fn the_ledger_lists_its_principals_and_their_active_grants() {
        let mut ledger = ledger_with_capable_ephemeral();
        ledger
            .allow(
                app("csv-comparator"),
                AppPermission::read(scope("~/Downloads/**")),
                Actor::User,
                "yes",
            )
            .unwrap();

        let principals = ledger.principals();
        assert!(principals.contains(&Principal::Ephemeral));
        assert!(principals.contains(&app("csv-comparator")));
        assert_eq!(principals.len(), 2);

        assert_eq!(ledger.active_grants(&app("csv-comparator")).len(), 1);
    }

    #[test]
    fn the_ledger_round_trips_through_json() {
        let mut ledger = ledger_with_capable_ephemeral();
        ledger
            .allow(
                app("csv-comparator"),
                AppPermission::read(scope("~/Downloads/**")),
                Actor::User,
                "yes",
            )
            .unwrap();
        ledger.revoke_all(&app("csv-comparator"));

        let json = serde_json::to_string(&ledger).unwrap();
        let restored: PermissionLedger = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, ledger);
        assert_eq!(
            restored.check(&app("csv-comparator"), &read("~/Downloads/a.csv")),
            Decision::Deny,
            "a revocation must survive a round trip"
        );
    }
}
