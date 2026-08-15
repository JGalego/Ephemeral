//! How a permission question is put to a person.
//!
//! Ephemeral will not ship a dialog that says *"Allow filesystem access?"*. That
//! question is unanswerable: it does not say who is asking, what they want, or
//! what happens either way, so the only rational responses are "always yes" and
//! "always no" — and users pick the first.
//!
//! A [`PermissionPrompt`] is a structured answer to the five questions a person
//! actually needs answered, produced by the core from the permission itself. An
//! interface cannot render a meaningless prompt, because it is not given the
//! materials to build one.
//!
//! > **Apartment Comparator wants to read the files in `~/Downloads/apartments`.**
//! >
//! > It needs this to compare the CSV files you selected.
//! >
//! > If you allow it: it can read what is at `~/Downloads/apartments`. It cannot
//! > change those files, and it cannot see anything else on this device.
//! >
//! > You can take this back at any time from the app's page.
//! >
//! > \[Allow]  \[Deny]

use std::fmt;

use serde::{Deserialize, Serialize};

use super::Permission;
use crate::identity::Principal;

/// How dangerous a permission is.
///
/// Ordered, so the worst capability in a set can be found with `max`, and so a
/// narrow scope is provably less risky than a broad one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Little that could go wrong.
    Low,
    /// Worth reading before deciding.
    Medium,
    /// Could expose personal data or meaningfully widen what code can reach.
    High,
    /// Could undermine the other protections around this app or this device.
    Critical,
}

impl RiskLevel {
    /// Whether granting this requires an explicit, unambiguous confirmation
    /// rather than a default-highlighted button.
    #[must_use]
    pub fn requires_explicit_confirmation(self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    /// The machine-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything an interface needs to ask a permission question honestly.
///
/// The five fields correspond to the five questions from the product brief, and
/// there is deliberately no free-form "message" field: a prompt is assembled
/// from these parts, so no interface can substitute its own vaguer wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPrompt {
    /// **What is asking?** The principal, machine-readable.
    pub principal: Principal,

    /// **What is asking?** The name a person would recognise — an app's title,
    /// or "Ephemeral".
    pub asker: String,

    /// What is being asked for, machine-readable.
    pub permission: Permission,

    /// **What does it want?** In plain language, completing "… wants to".
    pub wants: String,

    /// **Why does it need it?** Supplied by whatever is requesting, in the
    /// user's terms.
    ///
    /// A request with no rationale is a request a person cannot evaluate, so
    /// this is required rather than optional.
    pub why: String,

    /// **What happens if I allow it?** Including what remains denied.
    pub if_allowed: String,

    /// **Can I revoke it later?** Always true in Ephemeral, and stated rather
    /// than assumed.
    pub revocable: bool,

    /// How emphatically to ask.
    pub risk: RiskLevel,
}

impl PermissionPrompt {
    /// Builds a prompt for a permission request.
    ///
    /// `asker` is the name a person would recognise, and `why` is the rationale
    /// in the user's terms — "to compare the CSV files you selected", not "the
    /// plan requires filesystem access".
    pub fn new(
        principal: Principal,
        asker: impl Into<String>,
        permission: impl Into<Permission>,
        why: impl Into<String>,
    ) -> Self {
        let permission = permission.into();
        Self {
            wants: permission.describe(),
            if_allowed: permission.consequences(),
            risk: permission.risk(),
            revocable: true,
            principal,
            asker: asker.into(),
            permission,
            why: why.into(),
        }
    }

    /// The headline, as a sentence.
    ///
    /// *"Apartment Comparator wants to read the files in `~/Downloads/apartments`."*
    #[must_use]
    pub fn headline(&self) -> String {
        format!("{} wants to {}.", self.asker, self.wants)
    }

    /// Whether this prompt must be confirmed explicitly rather than accepted
    /// with a default action.
    #[must_use]
    pub fn requires_explicit_confirmation(&self) -> bool {
        self.risk.requires_explicit_confirmation()
    }

    /// The whole prompt as text, for the CLI and for tests.
    ///
    /// Interfaces with more room should render the fields themselves; this is
    /// the reference rendering, and it is what proves every field is populated.
    #[must_use]
    pub fn render(&self) -> String {
        let revocation = if self.revocable {
            "You can take this back at any time from the app's page."
        } else {
            "This cannot be taken back."
        };

        format!(
            "{}\n\n{}\n\nIf you allow it: {}\n\n{}",
            self.headline(),
            self.why,
            self.if_allowed,
            revocation
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::AppId;
    use crate::permission::{AppPermission, HostScope, MetaPermission, PathScope};

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    fn apartment_prompt() -> PermissionPrompt {
        PermissionPrompt::new(
            Principal::app(AppId::parse("apartment-comparator").unwrap()),
            "Apartment Comparator",
            AppPermission::read(scope("~/Downloads/apartments/**")),
            "It needs this to compare the CSV files you selected.",
        )
    }

    #[test]
    fn risk_levels_are_ordered_worst_last() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
        assert_eq!(
            [RiskLevel::Low, RiskLevel::Critical, RiskLevel::Medium]
                .into_iter()
                .max(),
            Some(RiskLevel::Critical)
        );
    }

    #[test]
    fn only_high_and_critical_demand_explicit_confirmation() {
        assert!(!RiskLevel::Low.requires_explicit_confirmation());
        assert!(!RiskLevel::Medium.requires_explicit_confirmation());
        assert!(RiskLevel::High.requires_explicit_confirmation());
        assert!(RiskLevel::Critical.requires_explicit_confirmation());
    }

    /// The prompt from the product brief, rendered.
    #[test]
    fn the_reference_prompt_reads_the_way_it_should() {
        let prompt = apartment_prompt();

        assert_eq!(
            prompt.headline(),
            "Apartment Comparator wants to read the files in ~/Downloads/apartments."
        );

        let rendered = prompt.render();
        assert!(rendered.contains("compare the CSV files you selected"));
        assert!(rendered.contains("It cannot change those files"));
        assert!(rendered.contains("take this back at any time"));
    }

    /// Every one of the five questions must be answered, and none of them with
    /// a placeholder.
    #[test]
    fn every_prompt_answers_all_five_questions() {
        let prompts = [
            apartment_prompt(),
            PermissionPrompt::new(
                Principal::Ephemeral,
                "Ephemeral",
                MetaPermission::UseDocker,
                "It needs this to run your apps in containers.",
            ),
            PermissionPrompt::new(
                Principal::app(AppId::parse("scraper").unwrap()),
                "Scraper",
                AppPermission::outbound(HostScope::parse("*.example.com").unwrap()),
                "It needs this to fetch the pages you asked about.",
            ),
        ];

        for prompt in prompts {
            assert!(!prompt.asker.is_empty(), "who is asking is unanswered");
            assert!(prompt.wants.len() > 5, "what it wants is unanswered");
            assert!(prompt.why.len() > 10, "why is unanswered");
            assert!(prompt.if_allowed.len() > 20, "consequences are unanswered");
            assert!(
                prompt.revocable,
                "Ephemeral permissions are always revocable"
            );
            assert!(
                prompt.headline().ends_with('.'),
                "the headline should read as a sentence: {}",
                prompt.headline()
            );
        }
    }

    /// A prompt for a dangerous capability must be marked so an interface knows
    /// not to offer it as the default action.
    #[test]
    fn dangerous_requests_demand_an_explicit_decision() {
        let dangerous = PermissionPrompt::new(
            Principal::app(AppId::parse("risky").unwrap()),
            "Risky",
            AppPermission::ExecuteProcesses,
            "It says it needs to run other programs.",
        );
        assert_eq!(dangerous.risk, RiskLevel::Critical);
        assert!(dangerous.requires_explicit_confirmation());

        assert!(
            !apartment_prompt().requires_explicit_confirmation(),
            "reading two CSV files the user chose should not need a scary dialog"
        );
    }

    /// A meta-permission prompt must say that generated apps do not inherit it,
    /// because that is the thing a user most reasonably fears.
    #[test]
    fn a_meta_prompt_says_apps_do_not_inherit_it() {
        let prompt = PermissionPrompt::new(
            Principal::Ephemeral,
            "Ephemeral",
            MetaPermission::read(scope("~/**")),
            "It needs this to find the files you point it at.",
        );
        assert!(prompt.if_allowed.contains("separate permissions"));
    }

    #[test]
    fn prompts_round_trip_through_json() {
        let prompt = apartment_prompt();
        let json = serde_json::to_string(&prompt).unwrap();
        assert_eq!(
            serde_json::from_str::<PermissionPrompt>(&json).unwrap(),
            prompt
        );
    }
}
