//! `ephemeral review` — decide what an application may do, one question at a
//! time.
//!
//! The permission prompt has existed since Phase 0 and nothing reached it. This
//! is what reaches it. Every question answers the five things a person needs in
//! order to decide, which is the whole reason [`PermissionPrompt`] has the
//! fields it does:
//!
//! 1. What is asking?
//! 2. What does it want?
//! 3. Why does it say it needs it?
//! 4. What happens if I allow it?
//! 5. Can I take it back?
//!
//! Two rules hold here and are worth stating because they are easy to erode:
//!
//! - **Nothing is granted without an answer.** There is no default action, no
//!   "allow all", and no timeout that decides for you. When there is nobody to
//!   ask — no terminal — this prints what it *would* ask and grants nothing.
//! - **A high-risk permission cannot be accepted by reflex.** It takes the word
//!   `allow`, not a keystroke, because a `y` habit formed on low-risk questions
//!   should not carry over to the one that matters.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use anyhow::{Context as _, Result};
use ephemeral_core::{
    Actor, AppManifest, Principal,
    audit::AuditEvent,
    permission::{AppPermission, Decision, Permission, PermissionPrompt, RiskLevel},
    storage::Workspace,
};

use crate::output;

/// What a person said about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Allow,
    Deny,
    Skip,
}

/// Walks through everything an application has asked for and not been given.
pub(crate) fn run(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let manifest = crate::commands::find(&workspace, reference)?;
    let subject = Principal::app(manifest.id.clone());

    let outstanding = outstanding(&workspace, &manifest);

    if outstanding.is_empty() {
        println!(
            "{} {} has no requests waiting on you.",
            output::good("Nothing to review."),
            manifest.id
        );
        println!(
            "{}",
            output::dim(&format!(
                "`ephemeral permissions {}` shows what it has and what it wants.",
                manifest.id
            ))
        );
        return Ok(());
    }

    println!(
        "{}",
        output::heading(&format!(
            "{} — {} to decide",
            manifest.name,
            outstanding.len()
        ))
    );

    // Nobody to ask. Printing the questions is useful; answering them on
    // somebody's behalf is not, so this grants nothing.
    if !std::io::stdin().is_terminal() {
        show_without_asking(&manifest, &outstanding);
        return Ok(());
    }

    let mut allowed = 0;
    let mut denied = 0;

    for permission in &outstanding {
        let prompt = prompt_for(&manifest, permission);

        println!();
        println!("{}", output::risk(prompt.risk));
        println!("{}", prompt.render());
        println!();

        match ask(&prompt)? {
            Answer::Allow => {
                workspace
                    .ledger_mut()
                    .allow(
                        subject.clone(),
                        Permission::App(permission.clone()),
                        Actor::User,
                        &prompt.why,
                    )
                    .with_context(|| format!("could not record the decision about {permission}"))?;
                record(&mut workspace, &subject, permission, Decision::Allow);
                allowed += 1;
                println!("{}", output::good("Allowed."));
            }
            Answer::Deny => {
                workspace
                    .ledger_mut()
                    .deny(
                        subject.clone(),
                        Permission::App(permission.clone()),
                        Actor::User,
                        "declined during review",
                    )
                    .with_context(|| format!("could not record the decision about {permission}"))?;
                record(&mut workspace, &subject, permission, Decision::Deny);
                denied += 1;
                println!("{}", output::good("Denied."));
            }
            Answer::Skip => println!(
                "{}",
                output::dim("Left undecided, which means it stays denied.")
            ),
        }
    }

    workspace.save()?;
    summarise(&workspace, &manifest, &outstanding, allowed, denied);
    Ok(())
}

/// Prints the questions without answering any of them.
fn show_without_asking(manifest: &AppManifest, outstanding: &[AppPermission]) {
    println!();
    for permission in outstanding {
        println!("{}", prompt_for(manifest, permission).render());
        println!();
    }
    println!(
        "{}",
        output::dim(
            "Nothing was decided: there is no terminal to ask. Run this interactively, or use \
             `ephemeral grant` and `ephemeral revoke` one at a time."
        )
    );
}

/// Says what was decided, and what will still not work.
fn summarise(
    workspace: &Workspace,
    manifest: &AppManifest,
    outstanding: &[AppPermission],
    allowed: usize,
    denied: usize,
) {
    println!();
    println!(
        "{} {allowed} allowed, {denied} denied, {} left undecided.",
        output::good("Done."),
        outstanding.len() - allowed - denied
    );
    println!(
        "{}",
        output::dim("Anything undecided stays denied, which is the default.")
    );

    // An allowed permission that still does nothing is worse than a denied one,
    // because the user believes it worked.
    let blocked = blocked_by_ephemeral(workspace, manifest, outstanding);
    if blocked.is_empty() {
        return;
    }

    println!();
    println!(
        "{} {} of these will not take effect yet.",
        output::warn("Note:"),
        blocked.len()
    );
    println!(
        "{}",
        output::dim(
            "Ephemeral itself has not been allowed to do that class of thing, and an \
             application never inherits Ephemeral's permissions. Run `ephemeral permissions \
             ephemeral` to see what it holds."
        )
    );
}

/// Everything the application asked for that has not been decided.
///
/// Deliberately the **application-level** decision, not the effective one.
/// `PermissionLedger::check_app` also requires that Ephemeral itself holds the
/// matching meta-permission, and conflating the two would ask a person about
/// their application when the thing actually missing was Ephemeral's own
/// authority — a question they cannot answer correctly because it is the wrong
/// question. The missing meta-permission is reported separately.
///
/// A permission already covered by a wider grant is not asked about again, and
/// neither is one already refused: being asked twice teaches people to stop
/// reading, and re-asking a denial makes "no" mean "not yet".
fn outstanding(workspace: &Workspace, manifest: &AppManifest) -> Vec<AppPermission> {
    let subject = Principal::app(manifest.id.clone());

    manifest
        .permissions
        .capabilities()
        .into_iter()
        .filter(|permission| {
            let decided = workspace
                .ledger()
                .active_grants(&subject)
                .iter()
                .any(|grant| {
                    grant
                        .permission
                        .satisfies(&Permission::App(permission.clone()))
                });

            !decided
        })
        .collect()
}

/// Meta-permissions Ephemeral itself lacks, without which a grant would do
/// nothing.
///
/// Reported rather than silently allowed to make a granted permission
/// ineffective. "I allowed it and it still cannot read my files" is exactly the
/// confusion the two-tier model creates if nobody explains it.
fn blocked_by_ephemeral(
    workspace: &Workspace,
    manifest: &AppManifest,
    permissions: &[AppPermission],
) -> Vec<AppPermission> {
    permissions
        .iter()
        .filter(|permission| {
            matches!(
                workspace.ledger().check_app(&manifest.id, permission),
                ephemeral_core::permission::EffectiveDecision::MetaDenied { .. }
            )
        })
        .cloned()
        .collect()
}

/// The question for one request.
fn prompt_for(manifest: &AppManifest, permission: &AppPermission) -> PermissionPrompt {
    // A request with no recorded reason says so. Inventing one would be the
    // single most damaging thing this code could do, because the reason is the
    // only part of the prompt a person cannot check for themselves.
    let why = manifest.reason_for(permission).map_or_else(
        || "It gave no reason for wanting this.".to_owned(),
        |reason| format!("It says: {reason}"),
    );

    PermissionPrompt::new(
        Principal::app(manifest.id.clone()),
        &manifest.name,
        permission.clone(),
        why,
    )
}

/// Asks one question and reads the answer.
fn ask(prompt: &PermissionPrompt) -> Result<Answer> {
    let expected = if prompt.requires_explicit_confirmation() {
        "type `allow` to allow, `deny` to refuse, or press enter to skip"
    } else {
        "[y]es, [n]o, or press enter to skip"
    };

    loop {
        print!("{} ", output::dim(expected));
        std::io::stdout().flush().ok();

        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer)? == 0 {
            // End of input mid-review. Stopping is right; assuming an answer is
            // not.
            return Ok(Answer::Skip);
        }

        let answer = answer.trim().to_ascii_lowercase();

        if answer.is_empty() {
            return Ok(Answer::Skip);
        }

        match decide(&answer, prompt.risk) {
            Some(decision) => return Ok(decision),
            None => println!(
                "{}",
                output::dim("That was not one of the answers. Nothing has been decided yet.")
            ),
        }
    }
}

/// Reads one answer, given how emphatically the question was asked.
///
/// Separated from the reading so the rule that matters — a high-risk permission
/// cannot be accepted with a keystroke — is a pure function with a test.
fn decide(answer: &str, risk: RiskLevel) -> Option<Answer> {
    if risk.requires_explicit_confirmation() {
        return match answer {
            "allow" => Some(Answer::Allow),
            "deny" => Some(Answer::Deny),
            // Deliberately not accepting "y". A habit formed on low-risk
            // questions must not carry over to this one.
            _ => None,
        };
    }

    match answer {
        "y" | "yes" | "allow" => Some(Answer::Allow),
        "n" | "no" | "deny" => Some(Answer::Deny),
        _ => None,
    }
}

/// Writes the decision to the audit record.
fn record(
    workspace: &mut Workspace,
    subject: &Principal,
    permission: &AppPermission,
    decision: Decision,
) {
    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::PermissionDecided {
            principal: subject.clone(),
            permission: Permission::App(permission.clone()),
            decision,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{AppId, manifest::PermissionRationale, permission::PathScope};

    fn manifest() -> AppManifest {
        let mut manifest = AppManifest::requested(
            AppId::parse("csv-comparator").expect("a valid id"),
            "CSV comparator",
        );
        let permission =
            AppPermission::read(PathScope::parse("~/Downloads/**").expect("a valid scope"));
        manifest.permissions.request(&permission);
        manifest.rationale = vec![PermissionRationale {
            permission,
            reason: "to read the files you want compared".to_owned(),
        }];
        manifest
    }

    /// The rule this file exists to hold. A `y` habit formed on low-risk
    /// questions must not carry over to a critical one.
    #[test]
    fn a_high_risk_permission_cannot_be_accepted_with_a_keystroke() {
        for reflex in ["y", "yes", "", "ok", "sure"] {
            assert_ne!(
                decide(reflex, RiskLevel::Critical),
                Some(Answer::Allow),
                "{reflex:?} should not allow a critical permission"
            );
        }

        assert_eq!(decide("allow", RiskLevel::Critical), Some(Answer::Allow));
        assert_eq!(decide("deny", RiskLevel::Critical), Some(Answer::Deny));
    }

    /// A low-risk question can be answered quickly. Making everything hard
    /// makes nothing hard.
    #[test]
    fn a_low_risk_permission_takes_an_ordinary_answer() {
        assert_eq!(decide("y", RiskLevel::Low), Some(Answer::Allow));
        assert_eq!(decide("yes", RiskLevel::Low), Some(Answer::Allow));
        assert_eq!(decide("n", RiskLevel::Low), Some(Answer::Deny));
        assert_eq!(decide("no", RiskLevel::Low), Some(Answer::Deny));
    }

    /// Anything unrecognised decides nothing. A mistyped answer must never be
    /// read as consent.
    #[test]
    fn an_unrecognised_answer_decides_nothing() {
        for risk in [RiskLevel::Low, RiskLevel::Critical] {
            assert_eq!(decide("maybe", risk), None);
            assert_eq!(decide("yolo", risk), None);
        }
    }

    /// The prompt answers all five questions, including the one Ephemeral must
    /// never make up.
    #[test]
    fn a_prompt_answers_every_question_a_person_needs() {
        let manifest = manifest();
        let permission = &manifest.permissions.capabilities()[0];
        let prompt = prompt_for(&manifest, permission);
        let rendered = prompt.render();

        assert!(rendered.contains("CSV comparator"), "what is asking");
        assert!(rendered.contains("read"), "what it wants");
        assert!(rendered.contains("files you want compared"), "why");
        assert!(!prompt.if_allowed.is_empty(), "what happens if I allow it");
        assert!(rendered.contains("take this back"), "can I revoke it");
    }

    /// The reason is the one part of a prompt a person cannot check. Inventing
    /// one would be the most damaging thing this code could do.
    #[test]
    fn a_request_with_no_recorded_reason_says_so() {
        let mut manifest = manifest();
        manifest.rationale.clear();

        let permission = &manifest.permissions.capabilities()[0];
        let prompt = prompt_for(&manifest, permission);

        assert!(prompt.why.contains("gave no reason"), "{}", prompt.why);
    }

    /// The reason is presented as the application's claim, not as fact.
    #[test]
    fn the_reason_is_attributed_rather_than_asserted() {
        let manifest = manifest();
        let permission = &manifest.permissions.capabilities()[0];

        assert!(
            prompt_for(&manifest, permission)
                .why
                .starts_with("It says:"),
            "a model's reason is an assertion and must read like one"
        );
    }

    /// Being asked again for something already allowed teaches people to stop
    /// reading.
    #[test]
    fn something_already_decided_is_not_asked_about_again() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let mut workspace = Workspace::open(home.path()).expect("a workspace");
        let manifest = manifest();
        let permission = manifest.permissions.capabilities()[0].clone();

        assert_eq!(outstanding(&workspace, &manifest).len(), 1);

        workspace
            .ledger_mut()
            .allow(
                Principal::app(manifest.id.clone()),
                Permission::App(permission.clone()),
                Actor::User,
                "because I said so",
            )
            .expect("the user may grant");

        assert!(outstanding(&workspace, &manifest).is_empty());
    }

    /// "No" must not mean "not yet". Re-asking a denied permission is how a
    /// refusal gets worn down.
    #[test]
    fn something_already_denied_is_not_asked_about_again() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let mut workspace = Workspace::open(home.path()).expect("a workspace");
        let manifest = manifest();
        let permission = manifest.permissions.capabilities()[0].clone();

        workspace
            .ledger_mut()
            .deny(
                Principal::app(manifest.id.clone()),
                Permission::App(permission),
                Actor::User,
                "no",
            )
            .expect("the user may deny");

        assert!(outstanding(&workspace, &manifest).is_empty());
    }
}
