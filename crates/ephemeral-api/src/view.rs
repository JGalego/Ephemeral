//! What a client shows, as data.
//!
//! Every string in here is already phrased for a person — "read the files in
//! `~/Downloads`", not `filesystem_read`. That is deliberate: the alternative is
//! each client inventing its own wording, and a permission that reads one way in
//! a terminal and another in a window is two different promises.
//!
//! Nothing here is formatted for a particular medium. No colour, no escape
//! codes, no column widths. A client decides how to draw; this decides what
//! there is to draw.

use serde::{Deserialize, Serialize};

use ephemeral_core::{
    AppManifest, LifecycleState, Principal, Timestamp,
    audit::AuditEntry,
    permission::{AppPermission, PermissionLedger, RiskLevel},
};

/// One application, as it appears in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationSummary {
    /// Which application.
    pub id: String,

    /// The name a person recognises.
    pub name: String,

    /// What they asked for.
    pub purpose: String,

    /// Where it is in its life, in the user's language.
    pub state: String,

    /// The same, machine-readable, for a client that wants an icon.
    pub state_kind: String,

    /// Whether it could be started right now.
    pub runnable: bool,

    /// Whether it is holding a container.
    pub running: bool,

    /// Whether it has been archived or deleted.
    ///
    /// A flag rather than an exclusion: whether to show it is the client's
    /// decision, but which ones they are is not.
    pub put_away: bool,

    /// How many capabilities it has been given.
    pub granted: usize,

    /// The highest risk among what it holds, if it holds anything.
    ///
    /// A count alone does not separate "reads one folder" from "can reach the
    /// whole internet", and on a list those two looked identical — an
    /// application holding everything was drawn exactly like one that can see
    /// nothing of yours. Carried here so a client can draw the difference
    /// without recomputing it, which is how two clients start disagreeing about
    /// which applications are dangerous.
    pub highest_granted_risk: Option<String>,

    /// How many it has asked for and not been given.
    ///
    /// The number a client turns into a badge, because an application waiting
    /// on a decision is the thing a person most needs to notice.
    pub awaiting_decision: usize,

    /// When it last changed.
    pub updated_at: Timestamp,
}

impl ApplicationSummary {
    /// Builds a summary.
    #[must_use]
    pub fn of(manifest: &AppManifest, ledger: &PermissionLedger) -> Self {
        let state = manifest.lifecycle.state();
        let subject = Principal::app(manifest.id.clone());

        // Meta-permissions are Ephemeral's own authority and never an
        // application's, so they are no part of what an application holds.
        // The ledger already refuses to grant one against an app principal, so
        // this filter is not what enforces the separation — it mirrors
        // `PermissionsView` so that the count on the list and the list on the
        // page can never disagree about what an application holds.
        let held: Vec<RiskLevel> = ledger
            .active_grants(&subject)
            .iter()
            .filter(|grant| grant.decision.is_allowed())
            .filter_map(|grant| match &grant.permission {
                ephemeral_core::permission::Permission::App(permission) => Some(permission.risk()),
                ephemeral_core::permission::Permission::Meta(_) => None,
            })
            .collect();

        let highest_granted_risk = held.iter().max().map(|risk| risk.as_str().to_owned());

        Self {
            id: manifest.id.to_string(),
            name: manifest.name.clone(),
            purpose: manifest.metadata.purpose.clone(),
            state: state.headline().to_owned(),
            state_kind: format!("{:?}", state.kind()).to_lowercase(),
            runnable: state.is_runnable(),
            running: state.requires_runtime(),
            put_away: matches!(state, LifecycleState::Archived | LifecycleState::Deleted),
            granted: held.len(),
            highest_granted_risk,
            awaiting_decision: crate::outstanding_requests(manifest, ledger).len(),
            updated_at: manifest.updated_at,
        }
    }
}

/// One application's whole page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationDetail {
    /// Everything the list shows.
    pub summary: ApplicationSummary,

    /// What is happening, in a sentence.
    pub explanation: String,

    /// What the state means.
    pub description: String,

    /// Where it runs, if that has been decided.
    pub runtime: Option<RuntimeView>,

    /// What it may consume.
    pub limits: LimitsView,

    /// What it is allowed to do, and what it has asked for.
    pub permissions: PermissionsView,

    /// What it has been, newest first.
    pub versions: Vec<VersionView>,

    /// How long it is kept.
    pub retention: String,
}

impl ApplicationDetail {
    /// Builds a detail view.
    #[must_use]
    pub fn of(manifest: &AppManifest, ledger: &PermissionLedger) -> Self {
        let current = manifest
            .current_version()
            .map(|version| version.digest.clone());
        let mut versions: Vec<VersionView> = manifest
            .versions
            .iter()
            .map(|version| VersionView {
                digest: version.digest.short().to_owned(),
                sequence: version.sequence,
                reason: version.reason.clone(),
                created_at: version.created_at,
                current: current.as_ref() == Some(&version.digest),
                // Unknown here, and said so: this constructor has a ledger and
                // no store, and whether the bytes are still on disk is a
                // question only a store can answer.
                source_kept: None,
            })
            .collect();
        versions.reverse();

        Self {
            summary: ApplicationSummary::of(manifest, ledger),
            explanation: manifest.lifecycle.explain(),
            description: manifest.lifecycle.state().description().to_owned(),
            runtime: manifest.runtime.as_ref().map(|runtime| RuntimeView {
                kind: runtime.kind.to_string(),
                isolation: runtime.kind.describe_isolation().to_owned(),
                runs_locally: runtime.runs_locally(),
                image: runtime.image.clone(),
                interface: runtime.interface.to_string(),
                primary_action: runtime.interface.primary_action().to_owned(),
            }),
            limits: LimitsView {
                description: manifest.resources.describe(),
                cpu_millis: manifest.resources.cpu_millis,
                memory_mib: manifest.resources.memory_mib,
                storage_mib: manifest.resources.storage_mib,
            },
            permissions: PermissionsView::of(manifest, ledger),
            versions,
            retention: manifest.metadata.retention.headline().clone(),
        }
    }
}

/// Where an application runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeView {
    /// Which runtime.
    pub kind: String,

    /// What that confines, in the user's language.
    pub isolation: String,

    /// Whether the user's data stays on their machine.
    ///
    /// Carried separately from the prose so a client can make it prominent.
    /// This is the fact a person most needs and is least likely to read.
    pub runs_locally: bool,

    /// The image, if there is one.
    pub image: Option<String>,

    /// How a person uses it.
    pub interface: String,

    /// What the main button should say.
    pub primary_action: String,
}

/// What an application may consume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitsView {
    /// The whole thing in a sentence.
    pub description: String,

    /// CPU, in thousandths of a core.
    pub cpu_millis: u32,

    /// Memory ceiling, in mebibytes.
    pub memory_mib: u32,

    /// Disk ceiling, in mebibytes.
    pub storage_mib: u32,
}

/// One capability, as a person reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionView {
    /// The stable capability name, for a client that wants an icon.
    pub capability: String,

    /// What it wants, completing "… wants to".
    pub wants: String,

    /// Why, as stated by whatever asked. `None` when nothing was recorded.
    ///
    /// A client must present this as a claim and must not invent one, because
    /// the reason is the only part of a request a person cannot check.
    pub reason: Option<String>,

    /// What allowing it means, including what stays denied.
    pub if_allowed: String,

    /// How emphatically to ask.
    pub risk: String,

    /// Whether it needs more than a single click.
    pub needs_explicit_confirmation: bool,

    /// Whether it can be taken back. Always true, and stated rather than
    /// assumed.
    pub revocable: bool,
}

impl PermissionView {
    /// A capability an application has asked for.
    #[must_use]
    pub fn requested(
        permission: &AppPermission,
        manifest: &AppManifest,
        _ledger: &PermissionLedger,
    ) -> Self {
        Self {
            capability: permission.capability().to_owned(),
            wants: permission.describe(),
            reason: manifest.reason_for(permission).map(ToOwned::to_owned),
            if_allowed: permission.consequences(),
            risk: permission.risk().as_str().to_owned(),
            needs_explicit_confirmation: permission.risk().requires_explicit_confirmation(),
            revocable: true,
        }
    }
}

/// What an application has, and what it wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsView {
    /// What it has been given.
    pub allowed: Vec<PermissionView>,

    /// What it has asked for and not been given.
    pub outstanding: Vec<PermissionView>,

    /// The highest risk among what it holds, if it holds anything.
    pub highest_granted_risk: Option<String>,

    /// Whether it can currently reach anything of the user's at all.
    ///
    /// The sentence worth showing when it is false, because "this can see
    /// nothing of yours" is the common case and the reassuring one.
    pub isolated: bool,
}

impl PermissionsView {
    /// Builds the permissions view.
    #[must_use]
    pub fn of(manifest: &AppManifest, ledger: &PermissionLedger) -> Self {
        let subject = Principal::app(manifest.id.clone());

        let allowed: Vec<AppPermission> = ledger
            .active_grants(&subject)
            .iter()
            .filter(|grant| grant.decision.is_allowed())
            .filter_map(|grant| match &grant.permission {
                ephemeral_core::permission::Permission::App(permission) => Some(permission.clone()),
                // A meta-permission is Ephemeral's own authority and never an
                // application's, so it has no place on an application's page.
                ephemeral_core::permission::Permission::Meta(_) => None,
            })
            .collect();

        let highest_granted_risk = allowed
            .iter()
            .map(AppPermission::risk)
            .max()
            .map(|risk| risk.as_str().to_owned());

        Self {
            isolated: allowed.is_empty(),
            allowed: allowed
                .iter()
                .map(|permission| PermissionView::requested(permission, manifest, ledger))
                .collect(),
            outstanding: crate::outstanding_requests(manifest, ledger),
            highest_granted_risk,
        }
    }
}

/// One version of an application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionView {
    /// The abbreviated digest a person reads.
    pub digest: String,

    /// The human-facing sequence number.
    pub sequence: u32,

    /// Why it exists.
    pub reason: String,

    /// When it was produced.
    pub created_at: Timestamp,

    /// Whether this is the version the application is on now.
    ///
    /// Carried rather than inferred from position, because "the newest entry"
    /// and "the current version" stop being the same thing the moment somebody
    /// rolls back — the history keeps the version rolled away from, and a
    /// client that assumed otherwise would offer to return to the version it is
    /// already on.
    pub current: bool,

    /// Whether this version's source is still on this machine.
    ///
    /// `None` means nobody checked — the view was built without a store, which
    /// is a different statement from "the source is gone". A client deciding
    /// whether to offer a rollback should treat only `Some(true)` as yes;
    /// guessing at `None` is how a window offers to restore something that
    /// cannot be restored.
    pub source_kept: Option<bool>,
}

/// One entry from the security record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntryView {
    /// What happened, in the user's language.
    pub summary: String,

    /// Who caused it.
    pub actor: String,

    /// Which application, if it concerns one.
    pub app: Option<String>,

    /// When.
    pub at: Timestamp,
}

impl AuditEntryView {
    /// Builds an entry view.
    #[must_use]
    pub fn of(entry: &AuditEntry) -> Self {
        Self {
            summary: entry.event.describe(),
            actor: entry.actor.describe().to_owned(),
            app: entry.event.app().map(ToString::to_string),
            at: entry.at,
        }
    }
}

/// Risk levels, in the order a client should sort them.
#[must_use]
pub fn risk_order() -> Vec<String> {
    [
        RiskLevel::Low,
        RiskLevel::Medium,
        RiskLevel::High,
        RiskLevel::Critical,
    ]
    .into_iter()
    .map(|risk| risk.as_str().to_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{
        Actor, AppId,
        permission::{PathScope, Permission},
    };

    fn manifest() -> AppManifest {
        let mut manifest = AppManifest::requested(
            AppId::parse("csv-comparator").expect("a valid id"),
            "CSV comparator",
        );
        manifest.metadata.purpose = "compare two CSV files".to_owned();
        manifest
    }

    fn scope() -> PathScope {
        PathScope::parse("~/Downloads/**").expect("a valid scope")
    }

    /// "This can see nothing of yours" is the common case and the reassuring
    /// one, so a client has to be able to say it without working it out.
    #[test]
    fn an_application_with_no_grants_is_reported_as_isolated() {
        let view = PermissionsView::of(&manifest(), &PermissionLedger::new());

        assert!(view.isolated);
        assert!(view.allowed.is_empty());
        assert_eq!(view.highest_granted_risk, None);
    }

    #[test]
    fn a_granted_capability_appears_with_its_risk() {
        let manifest = manifest();
        let mut ledger = PermissionLedger::new();
        ledger
            .allow(
                Principal::app(manifest.id.clone()),
                Permission::App(AppPermission::read(scope())),
                Actor::User,
                "to compare them",
            )
            .expect("the user may grant");

        let view = PermissionsView::of(&manifest, &ledger);

        assert!(!view.isolated);
        assert_eq!(view.allowed.len(), 1);
        assert!(view.highest_granted_risk.is_some());
    }

    /// Ephemeral's own authority is not an application's, and must not appear
    /// on an application's page as though it were.
    #[test]
    fn a_meta_permission_never_appears_on_an_applications_page() {
        let manifest = manifest();
        let mut ledger = PermissionLedger::new();
        ledger
            .allow(
                Principal::Ephemeral,
                Permission::Meta(ephemeral_core::MetaPermission::UseDocker),
                Actor::User,
                "to run applications",
            )
            .expect("the user may grant");

        let view = PermissionsView::of(&manifest, &ledger);

        assert!(view.isolated, "the app still has nothing");
        assert!(view.allowed.is_empty());
    }

    /// The reason is the one part a person cannot check, so a view that has
    /// none says none rather than offering something.
    #[test]
    fn a_request_with_no_recorded_reason_carries_no_reason() {
        let mut manifest = manifest();
        manifest.permissions.request(&AppPermission::read(scope()));

        let requests = crate::outstanding_requests(&manifest, &PermissionLedger::new());

        assert_eq!(requests[0].reason, None);
        assert!(!requests[0].wants.is_empty());
    }

    /// Newest first, because that is the one being asked about.
    #[test]
    fn versions_are_shown_newest_first() {
        let mut manifest = manifest();
        let mut recipe = ephemeral_core::Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned()],
            source: vec![("main.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        recipe.normalise();
        manifest.record_version(&recipe, "generated");

        recipe.source = vec![("main.py".to_owned(), "bbb".to_owned())];
        recipe.normalise();
        manifest.record_version(&recipe, "repaired");

        let detail = ApplicationDetail::of(&manifest, &PermissionLedger::new());

        assert_eq!(detail.versions.len(), 2);
        assert_eq!(detail.versions[0].sequence, 2, "newest first");
        assert_eq!(detail.versions[0].reason, "repaired");
    }

    /// Whether data leaves the device is the fact a person most needs and is
    /// least likely to read, so it is a boolean rather than only prose.
    #[test]
    fn whether_an_application_runs_locally_is_a_fact_not_a_sentence() {
        let mut manifest = manifest();
        manifest.runtime = Some(ephemeral_core::manifest::RuntimeSpec::docker_job(
            "python:3.12-slim",
            vec!["python".to_owned()],
        ));

        let detail = ApplicationDetail::of(&manifest, &PermissionLedger::new());
        let runtime = detail.runtime.expect("a runtime was set");

        assert!(runtime.runs_locally);
        assert!(!runtime.isolation.is_empty());
        assert_eq!(runtime.primary_action, "Run once");
    }

    /// Views cross a process boundary in the desktop application, so they have
    /// to survive the trip.
    #[test]
    fn views_round_trip_through_json() {
        let detail = ApplicationDetail::of(&manifest(), &PermissionLedger::new());
        let json = serde_json::to_string(&detail).expect("serialisable");

        let parsed: ApplicationDetail = serde_json::from_str(&json).expect("readable");
        assert_eq!(parsed, detail);
    }

    #[test]
    fn risk_is_ordered_least_alarming_first() {
        let order = risk_order();

        assert_eq!(order.first().map(String::as_str), Some("low"));
        assert_eq!(order.last().map(String::as_str), Some("critical"));
    }
}
