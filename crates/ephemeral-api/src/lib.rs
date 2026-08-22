//! # Ephemeral API
//!
//! What every client asks Ephemeral to do, as data in and data out.
//!
//! The CLI and the desktop application are both clients of the same domain
//! model, and this is the surface they share. It exists so that a second client
//! is a *skin* rather than a second, subtly different Ephemeral — the failure
//! mode where a permission means one thing in a terminal and another in a
//! window ([ARCHITECTURE.md §5]).
//!
//! ## What is in here
//!
//! **Views**, in [`view`]: plain serialisable data describing what a client
//! should show, already phrased the way a person reads it. Nothing here
//! renders, formats for a terminal, or knows what a button is.
//!
//! **Operations**, in [`operation`]: the things a client asks Ephemeral to
//! *do*, whole. An operation is not a helper a client calls partway through —
//! creating an application means the manifest, its storage and the audit entry
//! together, because a client that did two of the three would produce an
//! application nobody is recorded as having asked for. That is not a
//! hypothetical: creation was written three times, and one copy had exactly
//! that hole.
//!
//! ## What is deliberately not in here
//!
//! **No decisions.** Nothing in this crate evaluates a permission, computes a
//! lifecycle transition, or joins a path. Those live in `ephemeral-core`, and a
//! service layer that reimplemented any of them would be exactly the second
//! Ephemeral it exists to prevent.
//!
//! **No I/O of its own.** Everything here works through a workspace the caller
//! opened and handed in — this crate names no path and opens no file — so it
//! compiles for every platform the core does, including the mobile ones that
//! have no runtime at all.
//!
//! **Nothing that needs a daemon or a network.** Generating talks to a model
//! and running needs a container, so both belong to the clients that have one.
//!
//! ## Versioning
//!
//! [`API_VERSION`] changes when a view changes shape in a way a client would
//! notice. A client is entitled to check it and refuse to run against a service
//! it does not understand, which is the whole reason it is a number rather than
//! an understanding.
//!
//! [ARCHITECTURE.md §5]: https://github.com/JGalego/Ephemeral/blob/main/ARCHITECTURE.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod authority;
pub mod operation;
pub mod view;

pub use authority::{Grants, Held};
pub use operation::{Rollback, create, derive_name, rollback, withdraw_widened};
pub use view::{
    ApplicationDetail, ApplicationSummary, AuditEntryView, LimitsView, PermissionView,
    PermissionsView, RuntimeView, VersionView,
};

use ephemeral_core::{
    AppId, AppManifest, Principal,
    audit::AuditLog,
    permission::{AppPermission, PermissionLedger},
    storage::Workspace,
};

/// The shape of the views in this crate.
///
/// Incremented when a client would notice a difference. A client that checks it
/// can refuse to run against a service it does not understand, rather than
/// misreading one.
pub const API_VERSION: u32 = 2;

/// Everything a client needs in order to draw a list of applications.
///
/// Sorted with the most recently touched first, because that is the one
/// somebody is looking for. Applications that are put away are included with a
/// flag rather than filtered out here — whether to show them is a client's
/// decision, but *which* are put away is not.
#[must_use]
pub fn applications(loaded: &[AppManifest], ledger: &PermissionLedger) -> Vec<ApplicationSummary> {
    let mut summaries: Vec<ApplicationSummary> = loaded
        .iter()
        .map(|manifest| ApplicationSummary::of(manifest, ledger))
        .collect();

    summaries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    summaries
}

/// Everything a client needs in order to draw one application's page.
///
/// The workspace is what separates this from [`ApplicationDetail::of`]: a
/// version's source can be recorded in the history and gone from the disk —
/// swept away by retention, or never kept at all because it predates snapshots
/// — and only the store knows which. A client that offered to return to a
/// version whose source is missing would offer something that cannot happen.
#[must_use]
pub fn application(manifest: &AppManifest, workspace: &Workspace) -> ApplicationDetail {
    let mut detail = ApplicationDetail::of(manifest, workspace.ledger());

    // Matched by digest against the manifest rather than by position, because
    // the view is reversed and a client of this crate should not have to know
    // that to read it.
    for view in &mut detail.versions {
        view.source_kept = manifest
            .versions
            .iter()
            .find(|version| version.digest.short() == view.digest)
            .map(|version| workspace.apps().has_version(&manifest.id, &version.digest));
    }

    detail
}

/// What an application has asked for and not been given.
///
/// The application-level decision, not the effective one: whether *Ephemeral*
/// holds the matching meta-permission is a separate question with a separate
/// answer, and conflating them asks a person something they cannot answer.
#[must_use]
pub fn outstanding_requests(
    manifest: &AppManifest,
    ledger: &PermissionLedger,
) -> Vec<PermissionView> {
    let subject = Principal::app(manifest.id.clone());

    manifest
        .permissions
        .capabilities()
        .into_iter()
        .filter(|permission| !decided(ledger, &subject, permission))
        .map(|permission| PermissionView::requested(&permission, manifest, ledger))
        .collect()
}

/// Whether the ledger already holds a decision covering this permission.
fn decided(ledger: &PermissionLedger, subject: &Principal, permission: &AppPermission) -> bool {
    let wanted = ephemeral_core::permission::Permission::App(permission.clone());

    ledger
        .active_grants(subject)
        .iter()
        .any(|grant| grant.permission.satisfies(&wanted))
}

/// The most recent security-record entries, newest first.
#[must_use]
pub fn recent_activity(log: &AuditLog, app: Option<&AppId>, limit: usize) -> Vec<AuditEntryView> {
    log.entries()
        .iter()
        .rev()
        .filter(|entry| app.is_none_or(|id| entry.event.app() == Some(id)))
        .take(limit)
        .map(AuditEntryView::of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{
        Actor,
        permission::{AppPermission, PathScope, Permission},
    };

    fn manifest(id: &str, name: &str) -> AppManifest {
        AppManifest::requested(AppId::parse(id).expect("a valid id"), name)
    }

    fn scope() -> PathScope {
        PathScope::parse("~/Downloads/**").expect("a valid scope")
    }

    /// The one somebody is looking for is the one they touched last.
    #[test]
    fn applications_are_listed_most_recently_touched_first() {
        let ledger = PermissionLedger::new();

        let older = manifest("older-app", "Older");
        let mut newer = manifest("newer-app", "Newer");
        newer.touch();

        let listed = applications(&[older, newer], &ledger);

        assert_eq!(listed[0].id, "newer-app");
        assert_eq!(listed[1].id, "older-app");
    }

    /// Whether to hide an archived application is a client's decision; which
    /// ones are archived is not.
    #[test]
    fn put_away_applications_are_flagged_rather_than_hidden() {
        let ledger = PermissionLedger::new();
        // Deleted rather than archived: archiving is not legal from Requested,
        // which the state machine is right about and I was not.
        let mut deleted = manifest("deleted-app", "Deleted");
        deleted
            .apply(ephemeral_core::lifecycle::TransitionRequest::new(
                ephemeral_core::LifecycleEvent::Delete,
                Actor::User,
                "thrown away",
            ))
            .expect("deleting is legal from anywhere a person can reach");

        let listed = applications(&[deleted], &ledger);

        assert_eq!(listed.len(), 1, "it is still there");
        assert!(listed[0].put_away, "and a client can tell");
    }

    /// A request already decided must not be offered again — being asked twice
    /// teaches people to stop reading.
    #[test]
    fn a_decided_request_is_no_longer_outstanding() {
        let mut manifest = manifest("csv-comparator", "CSV comparator");
        let permission = AppPermission::read(scope());
        manifest.permissions.request(&permission);

        let mut ledger = PermissionLedger::new();
        assert_eq!(outstanding_requests(&manifest, &ledger).len(), 1);

        ledger
            .allow(
                Principal::app(manifest.id.clone()),
                Permission::App(permission),
                Actor::User,
                "because I said so",
            )
            .expect("the user may grant");

        assert!(outstanding_requests(&manifest, &ledger).is_empty());
    }

    /// "No" must not mean "not yet".
    #[test]
    fn a_refused_request_is_no_longer_outstanding_either() {
        let mut manifest = manifest("csv-comparator", "CSV comparator");
        let permission = AppPermission::read(scope());
        manifest.permissions.request(&permission);

        let mut ledger = PermissionLedger::new();
        ledger
            .deny(
                Principal::app(manifest.id.clone()),
                Permission::App(permission),
                Actor::User,
                "no",
            )
            .expect("the user may deny");

        assert!(outstanding_requests(&manifest, &ledger).is_empty());
    }

    /// A view is data, not prose assembled for a terminal. Every client gets
    /// the same answer and decides how to draw it.
    #[test]
    fn views_carry_data_rather_than_rendered_output() {
        let mut manifest = manifest("csv-comparator", "CSV comparator");
        manifest.permissions.request(&AppPermission::read(scope()));

        let requests = outstanding_requests(&manifest, &PermissionLedger::new());
        let view = &requests[0];

        assert!(!view.wants.is_empty(), "what it wants, in the user's words");
        assert!(!view.if_allowed.is_empty(), "what allowing it means");
        assert!(view.revocable, "and that it can be taken back");
        assert!(
            !view.wants.contains("\\u{1b}"),
            "a view must not carry terminal formatting"
        );
    }

    /// A count alone does not separate "reads one folder" from "can reach the
    /// whole internet", and a list drawn from the count alone showed those two
    /// identically. Filming the desktop window is what made that visible.
    #[test]
    fn a_summary_reports_the_worst_thing_an_application_holds() {
        let mut manifest = manifest("csv-comparator", "CSV comparator");
        let reading = AppPermission::read(scope());
        let anywhere = AppPermission::outbound(
            ephemeral_core::permission::HostScope::parse("*").expect("anywhere is a valid scope"),
        );

        let mut ledger = PermissionLedger::new();
        let subject = Principal::app(manifest.id.clone());

        let empty = ApplicationSummary::of(&manifest, &ledger);
        assert_eq!(empty.granted, 0);
        assert_eq!(
            empty.highest_granted_risk, None,
            "an application holding nothing reports no risk, rather than a low one"
        );

        for permission in [&reading, &anywhere] {
            manifest.permissions.request(permission);
            ledger
                .allow(
                    subject.clone(),
                    Permission::App(permission.clone()),
                    Actor::User,
                    "because I said so",
                )
                .expect("the user may grant");
        }

        assert_eq!(
            ApplicationSummary::of(&manifest, &ledger).granted,
            0,
            "an application whose grants Ephemeral may not carry out holds nothing it can use"
        );

        // The other half of the model. Both are needed before a capability is
        // anything more than a record of a decision (ADR-0003).
        for permission in [&reading, &anywhere] {
            ledger
                .allow(
                    Principal::Ephemeral,
                    Permission::Meta(permission.required_meta()),
                    Actor::User,
                    "Ephemeral may carry these out",
                )
                .expect("the user may grant");
        }

        let summary = ApplicationSummary::of(&manifest, &ledger);

        assert_eq!(summary.granted, 2);
        assert_eq!(
            summary.highest_granted_risk.as_deref(),
            Some(anywhere.risk().as_str()),
            "the worst of them, not the first or the last"
        );
    }

    /// A meta-permission is Ephemeral's own authority and never an
    /// application's. Charging one to an application would report Ephemeral's
    /// risk against that application, on the very line a person reads to decide
    /// which of theirs is dangerous.
    ///
    /// The summary filters meta-permissions out, but that filter is not what
    /// makes this safe and this test does not pretend otherwise: the ledger
    /// refuses the grant outright, so the state the filter guards against
    /// cannot be reached through it. Asserting the refusal is asserting the
    /// thing that actually holds.
    #[test]
    fn ephemerals_own_authority_cannot_be_charged_to_an_application() {
        let manifest = manifest("csv-comparator", "CSV comparator");
        let mut ledger = PermissionLedger::new();

        let refused = ledger.allow(
            Principal::app(manifest.id.clone()),
            Permission::Meta(ephemeral_core::permission::MetaPermission::NetworkAccess),
            Actor::User,
            "Ephemeral may reach a model provider",
        );

        assert!(
            refused.is_err(),
            "an application must not be able to hold Ephemeral's own authority"
        );

        let summary = ApplicationSummary::of(&manifest, &ledger);
        assert_eq!(summary.granted, 0, "so it holds nothing");
        assert_eq!(summary.highest_granted_risk, None, "and risks nothing");
    }
}
