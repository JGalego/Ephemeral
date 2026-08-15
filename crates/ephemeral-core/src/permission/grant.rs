//! Decisions, and the record of who made them.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{AppPermission, MetaPermission, RiskLevel};
use crate::{Timestamp, actor::Actor, identity::Principal, now};

/// The answer to a permission question.
///
/// There is no third state. An absent grant is not "unknown", it is
/// [`Decision::Deny`] — permission checks are default-deny, and the absence of
/// evidence is treated as evidence of absence on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Permitted.
    Allow,
    /// Refused.
    Deny,
}

impl Decision {
    /// Whether this decision permits the operation.
    #[must_use]
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }

    /// The machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A permission in either space.
///
/// The two spaces are separate types precisely so they cannot be confused
/// ([ADR-0003]), and this enum is the only place they meet — a single container
/// so the ledger can hold both, with the ledger enforcing that Ephemeral holds
/// only [`Permission::Meta`] and an application holds only [`Permission::App`].
///
/// [ADR-0003]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0003-two-tier-permission-model.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "snake_case")]
pub enum Permission {
    /// Something Ephemeral itself may do.
    Meta(MetaPermission),

    /// Something one generated application may do.
    App(AppPermission),
}

impl Permission {
    /// Whether a grant of `self` covers a request for `requested`.
    ///
    /// **A permission never satisfies one from the other space.** This single
    /// line is what stops Ephemeral's own filesystem access from being read as
    /// authority for a generated app, and it is asserted directly by the
    /// security tests.
    #[must_use]
    pub fn satisfies(&self, requested: &Self) -> bool {
        match (self, requested) {
            (Self::Meta(held), Self::Meta(want)) => held.satisfies(want),
            (Self::App(held), Self::App(want)) => held.satisfies(want),
            (Self::Meta(_), Self::App(_)) | (Self::App(_), Self::Meta(_)) => false,
        }
    }

    /// How dangerous this permission is.
    #[must_use]
    pub fn risk(&self) -> RiskLevel {
        match self {
            Self::Meta(permission) => permission.risk(),
            Self::App(permission) => permission.risk(),
        }
    }

    /// What holding this permission allows, in plain language.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Meta(permission) => permission.describe(),
            Self::App(permission) => permission.describe(),
        }
    }

    /// What allowing this permission means, and what remains denied.
    #[must_use]
    pub fn consequences(&self) -> String {
        match self {
            Self::Meta(permission) => permission.consequences(),
            Self::App(permission) => permission.consequences(),
        }
    }

    /// Whether this principal is even eligible to hold this permission.
    ///
    /// Ephemeral holds meta-permissions; applications and plugins hold
    /// application permissions. Anything else is a programming error rather than
    /// a policy question, and the ledger refuses it.
    #[must_use]
    pub fn is_valid_for(&self, principal: &Principal) -> bool {
        matches!(
            (self, principal),
            (Self::Meta(_), Principal::Ephemeral)
                | (
                    Self::App(_),
                    Principal::App { .. } | Principal::Plugin { .. }
                )
        )
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Meta(permission) => write!(f, "meta:{permission}"),
            Self::App(permission) => write!(f, "app:{permission}"),
        }
    }
}

impl From<MetaPermission> for Permission {
    fn from(permission: MetaPermission) -> Self {
        Self::Meta(permission)
    }
}

impl From<AppPermission> for Permission {
    fn from(permission: AppPermission) -> Self {
        Self::App(permission)
    }
}

/// A recorded permission decision.
///
/// Grants are the ledger's entries. They are never edited: revoking one sets
/// [`Grant::revoked_at`] rather than removing it, so "this was allowed on Monday
/// and revoked on Tuesday" stays answerable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Who holds it.
    pub subject: Principal,

    /// What was decided about.
    pub permission: Permission,

    /// What was decided.
    pub decision: Decision,

    /// Who decided. Always [`Actor::User`] — the ledger refuses anything else.
    pub granted_by: Actor,

    /// When the decision was made.
    pub granted_at: Timestamp,

    /// When it stops applying by itself, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Timestamp>,

    /// When it was revoked, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<Timestamp>,

    /// Why the decision was made, in the user's terms.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Grant {
    /// Records a decision.
    pub fn new(
        subject: Principal,
        permission: impl Into<Permission>,
        decision: Decision,
        granted_by: Actor,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            subject,
            permission: permission.into(),
            decision,
            granted_by,
            granted_at: now(),
            expires_at: None,
            revoked_at: None,
            reason: reason.into(),
        }
    }

    /// Sets an expiry.
    #[must_use]
    pub fn expiring_at(mut self, at: Timestamp) -> Self {
        self.expires_at = Some(at);
        self
    }

    /// Whether this grant applies at a given moment.
    ///
    /// A revoked grant never applies again, and an expired one stops applying
    /// without anyone having to sweep it.
    #[must_use]
    pub fn is_active_at(&self, at: Timestamp) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        match self.expires_at {
            Some(expiry) => at < expiry,
            None => true,
        }
    }

    /// Whether this grant has been revoked.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// A one-line account for the audit log and the app's detail page.
    #[must_use]
    pub fn explain(&self) -> String {
        let verb = match self.decision {
            Decision::Allow => "may",
            Decision::Deny => "may not",
        };
        let status = if self.is_revoked() { " (revoked)" } else { "" };
        format!(
            "{} {verb} {}{status}",
            self.subject.label(),
            self.permission.describe()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::AppId;
    use crate::permission::PathScope;
    use chrono::Duration;

    fn app() -> Principal {
        Principal::app(AppId::parse("csv-comparator").unwrap())
    }

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    #[test]
    fn decisions_are_binary() {
        assert!(Decision::Allow.is_allowed());
        assert!(!Decision::Deny.is_allowed());
    }

    /// The invariant this enum exists to hold: a meta-permission is never
    /// authority for an application, and vice versa.
    #[test]
    fn the_two_permission_spaces_never_satisfy_each_other() {
        let meta = Permission::Meta(MetaPermission::read(scope("~/**")));
        let app_permission = Permission::App(AppPermission::read(scope("~/Downloads/a.csv")));

        assert!(
            !meta.satisfies(&app_permission),
            "Ephemeral's own filesystem access must never authorise an app"
        );
        assert!(!app_permission.satisfies(&meta));
        assert!(meta.satisfies(&Permission::Meta(MetaPermission::read(scope("~/a")))));
    }

    /// Only Ephemeral can hold a meta-permission, and only apps and plugins can
    /// hold application permissions.
    #[test]
    fn permissions_belong_to_one_kind_of_principal() {
        let meta = Permission::Meta(MetaPermission::UseDocker);
        let app_permission = Permission::App(AppPermission::Camera);

        assert!(meta.is_valid_for(&Principal::Ephemeral));
        assert!(!meta.is_valid_for(&app()));

        assert!(app_permission.is_valid_for(&app()));
        assert!(!app_permission.is_valid_for(&Principal::Ephemeral));
    }

    #[test]
    fn a_fresh_grant_is_active_and_a_revoked_one_never_is() {
        let mut grant = Grant::new(
            app(),
            AppPermission::read(scope("~/Downloads/**")),
            Decision::Allow,
            Actor::User,
            "to compare the files you chose",
        );
        assert!(grant.is_active_at(now()));

        grant.revoked_at = Some(now());
        assert!(!grant.is_active_at(now()));
        assert!(
            !grant.is_active_at(grant.granted_at),
            "revocation is not retroactive in effect, but a revoked grant never applies again"
        );
    }

    #[test]
    fn an_expiring_grant_stops_applying_without_a_sweep() {
        let expiry = now() + Duration::hours(1);
        let grant = Grant::new(
            app(),
            AppPermission::Camera,
            Decision::Allow,
            Actor::User,
            "for this session",
        )
        .expiring_at(expiry);

        assert!(grant.is_active_at(now()));
        assert!(!grant.is_active_at(expiry));
        assert!(!grant.is_active_at(expiry + Duration::seconds(1)));
    }

    #[test]
    fn grants_explain_themselves() {
        let allowed = Grant::new(
            app(),
            AppPermission::read(scope("~/Downloads/apartments/**")),
            Decision::Allow,
            Actor::User,
            "to compare the CSV files you selected",
        );
        assert_eq!(
            allowed.explain(),
            "app:csv-comparator may read the files in ~/Downloads/apartments"
        );

        let denied = Grant::new(
            app(),
            AppPermission::outbound(super::super::HostScope::parse("*").unwrap()),
            Decision::Deny,
            Actor::User,
            "no reason to send my data anywhere",
        );
        assert!(denied.explain().contains("may not"));
    }

    #[test]
    fn grants_round_trip_through_json() {
        let grant = Grant::new(
            app(),
            AppPermission::read(scope("~/Downloads/**")),
            Decision::Allow,
            Actor::User,
            "because you chose those files",
        );
        let json = serde_json::to_string(&grant).unwrap();
        assert_eq!(serde_json::from_str::<Grant>(&json).unwrap(), grant);
    }
}
