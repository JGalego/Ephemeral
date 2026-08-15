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
//! Views. Nothing else. Every type is plain serialisable data describing what a
//! client should show, already phrased the way a person reads it. Nothing here
//! renders, formats for a terminal, or knows what a button is.
//!
//! ## What is deliberately not in here
//!
//! **No decisions.** Nothing in this crate evaluates a permission, computes a
//! lifecycle transition, or joins a path. Those live in `ephemeral-core`, and a
//! service layer that reimplemented any of them would be exactly the second
//! Ephemeral it exists to prevent.
//!
//! **No I/O.** This crate builds views from a workspace a caller already
//! opened. It performs no host I/O of its own, so it compiles for every
//! platform the core does, including the mobile ones that have no runtime at
//! all.
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

pub mod view;

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
pub const API_VERSION: u32 = 1;

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
#[must_use]
pub fn application(manifest: &AppManifest, workspace: &Workspace) -> ApplicationDetail {
    ApplicationDetail::of(manifest, workspace.ledger())
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
}
