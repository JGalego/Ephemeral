//! What each command does.
//!
//! Every one of these goes through `ephemeral-core`. Nothing here decides a
//! permission question, computes a lifecycle transition, or touches a path
//! directly — the CLI is a client, and a client that reimplemented any of that
//! would be a second, subtly different Ephemeral.

use anyhow::{Context as _, Result, bail};
use ephemeral_core::{
    Actor, AppId, Principal,
    audit::{AuditEvent, AuditLog},
    lifecycle::{LifecycleEvent, LifecycleState, TransitionRequest},
    manifest::{AppManifest, Metadata},
    permission::{Permission, RiskLevel},
    retention::{RetentionPeriod, RetentionPolicy},
    storage::{AppStore as _, Workspace},
};
use std::path::Path;

use crate::{output, parse};

/// Opens the workspace, saying where it looked if that fails.
pub(crate) fn open(home: &Path) -> Result<Workspace> {
    Workspace::open(home)
        .with_context(|| format!("could not open Ephemeral's files at {}", home.display()))
}

/// Finds an application, and says something useful when it is not there.
pub(crate) fn find(workspace: &Workspace, reference: &str) -> Result<AppManifest> {
    let id = AppId::parse(reference)
        .with_context(|| format!("{reference:?} is not an application id"))?;

    workspace.apps().load(&id).with_context(|| {
        format!("no application called {reference}. Run `ephemeral list` to see what is here")
    })
}

/// Creates an application record from an intent.
pub(crate) fn create(home: &Path, intent: &str, name: Option<&str>, retention: &str) -> Result<()> {
    let intent = intent.trim();
    if intent.is_empty() {
        bail!("tell me what you want the application to do");
    }

    let policy = parse_retention(retention)?;
    let name = name.map_or_else(|| derive_name(intent), str::to_owned);
    let id = AppId::generate(&name);

    let mut manifest = AppManifest::requested(id.clone(), &name);
    intent.clone_into(&mut manifest.description);
    manifest.metadata = Metadata::for_intent(intent, policy);

    let mut workspace = open(home)?;
    workspace
        .apps_mut()
        .create(&manifest)
        .with_context(|| format!("could not save {id}"))?;
    workspace
        .apps()
        .prepare(&id)
        .with_context(|| format!("could not create the storage for {id}"))?;

    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppCreated {
            app: id.clone(),
            purpose: intent.to_owned(),
        },
    );
    workspace.save()?;

    println!("{} {}", output::good("Created"), output::bold(&name));
    println!("{}", output::field("id", id.as_str()));
    println!(
        "{}",
        output::field("state", &output::state(manifest.lifecycle.state()))
    );
    println!(
        "{}",
        output::field("retention", &policy.headline().to_lowercase())
    );
    println!();
    println!("{}", LifecycleState::Requested.description());
    println!();
    println!(
        "{}",
        output::dim(
            "Generation arrives in Phase 2, so nothing is being built yet. Everything else \
             works: try `ephemeral inspect`, `ephemeral grant`, `ephemeral logs`."
        )
    );

    Ok(())
}

/// Lists applications.
pub(crate) fn list(home: &Path, all: bool) -> Result<()> {
    let workspace = open(home)?;
    let result = workspace.load_all()?;

    let visible: Vec<_> = result
        .loaded
        .iter()
        .filter(|manifest| all || !is_put_away(manifest.lifecycle.state()))
        .collect();

    if visible.is_empty() && result.broken.is_empty() {
        // "No applications yet" would be a lie when there are archived or
        // deleted ones — the user would think their app had vanished.
        if result.loaded.is_empty() {
            println!("{}", output::dim("No applications yet."));
            println!();
            println!("Ask for one:  ephemeral create \"compare these two CSV files\"");
        } else {
            println!(
                "{}",
                output::dim(&format!(
                    "Nothing active. {} archived or deleted — show them with --all.",
                    result.loaded.len()
                ))
            );
        }
        return Ok(());
    }

    for manifest in &visible {
        println!(
            "{:<26} {:<22} {}",
            output::bold(manifest.id.as_str()),
            output::state(manifest.lifecycle.state()),
            output::dim(&output::relative(manifest.updated_at)),
        );
        if !manifest.metadata.purpose.is_empty() {
            println!("  {}", output::dim(&manifest.metadata.purpose));
        }
    }

    let hidden = result.loaded.len() - visible.len();
    if hidden > 0 && !all {
        println!();
        println!(
            "{}",
            output::dim(&format!(
                "{hidden} archived or deleted. Show them with --all."
            ))
        );
    }

    // Broken applications are reported rather than hidden: a manifest that
    // cannot be read is exactly the thing somebody needs to know about.
    if !result.broken.is_empty() {
        println!();
        for (id, problem) in &result.broken {
            println!("{} {id}: {problem}", output::bad("unreadable"));
        }
        println!("{}", output::dim("Run `ephemeral doctor` for more."));
    }

    Ok(())
}

/// Shows everything about one application.
pub(crate) fn inspect(home: &Path, reference: &str) -> Result<()> {
    let workspace = open(home)?;
    let manifest = find(&workspace, reference)?;
    let state = manifest.lifecycle.state();

    println!("{}", output::heading(&manifest.name));
    println!("{}", output::field("id", manifest.id.as_str()));
    println!("{}", output::field("state", &output::state(state)));
    println!(
        "{}",
        output::field("version", &manifest.version.to_string())
    );
    println!(
        "{}",
        output::field("created", &output::relative(manifest.created_at))
    );
    println!(
        "{}",
        output::field("updated", &output::relative(manifest.updated_at))
    );
    println!(
        "{}",
        output::field(
            "retention",
            &manifest.metadata.retention.headline().to_lowercase()
        )
    );

    if !manifest.metadata.purpose.is_empty() {
        println!();
        println!("{}", output::dim("You asked for"));
        println!("  {}", manifest.metadata.purpose);
    }

    println!();
    println!("{}", output::dim("What is happening"));
    println!("  {}", manifest.lifecycle.explain());

    println!();
    println!("{}", output::dim("Where it runs"));
    match &manifest.runtime {
        Some(runtime) => {
            println!("  {}", runtime.kind.describe_isolation());
            if let Some(image) = &runtime.image {
                println!("{}", output::field("image", image));
            }
        }
        None => println!(
            "  {}",
            output::dim("Not decided yet — planning is what settles this.")
        ),
    }
    println!("  {}", manifest.metadata.execution.describe());

    println!();
    println!("{}", output::dim("Limits"));
    println!("  {}", manifest.resources.describe());
    println!("  {}", manifest.budget.describe());

    println!();
    print_permissions(&workspace, &Principal::app(manifest.id.clone()));

    println!();
    println!(
        "{}",
        output::dim(&format!(
            "History: {} transition(s). See them with `ephemeral logs {}`.",
            manifest.lifecycle.history().len(),
            manifest.id
        ))
    );

    Ok(())
}

/// Shows what a principal is allowed to do.
pub(crate) fn permissions(home: &Path, reference: &str) -> Result<()> {
    let workspace = open(home)?;
    let subject = parse::principal(reference)?;

    if let Some(id) = subject.as_app() {
        workspace.apps().load(id).with_context(|| {
            format!("no application called {reference}. Run `ephemeral list` to see what is here")
        })?;
    }

    println!("{}", output::heading(&subject.label()));
    println!();
    print_permissions(&workspace, &subject);
    Ok(())
}

fn print_permissions(workspace: &Workspace, subject: &Principal) {
    let grants = workspace.ledger().active_grants(subject);

    println!("{}", output::dim("Allowed to"));
    if grants.is_empty() {
        println!(
            "  {}",
            output::dim("Nothing. Permissions have to be granted one at a time.")
        );
        return;
    }

    for grant in grants {
        let mark = if grant.decision.is_allowed() {
            output::good("allow")
        } else {
            output::bad("deny ")
        };
        println!(
            "  {mark} {:<44} {}",
            grant.permission.describe(),
            output::dim(&format!("[{}]", output::risk(grant.permission.risk())))
        );
        if !grant.reason.is_empty() {
            println!("        {}", output::dim(&grant.reason));
        }
    }
}

/// Grants a permission.
pub(crate) fn grant(
    home: &Path,
    reference: &str,
    permission: &str,
    why: Option<&str>,
) -> Result<()> {
    let mut workspace = open(home)?;
    let subject = parse::principal(reference)?;

    if let Some(id) = subject.as_app() {
        let manifest = workspace.apps().load(id).with_context(|| {
            format!("no application called {reference}. Run `ephemeral list` to see what is here")
        })?;
        // A deleted application has had its permissions withdrawn; granting one
        // back would quietly bring it partway to life.
        if manifest.lifecycle.state() == LifecycleState::Deleted {
            bail!("{reference} is deleted. Restore it first if you want it to do anything again");
        }
    }

    let permission = parse::permission(&subject, permission)?;
    let reason = why.unwrap_or("granted from the command line");

    // Say what is being agreed to before agreeing to it, in the same terms the
    // permission UI will use.
    println!(
        "{} wants to {}.",
        output::bold(&subject.label()),
        permission.describe()
    );
    println!();
    println!("If you allow it: {}", permission.consequences());
    if permission.risk().requires_explicit_confirmation() {
        println!();
        println!(
            "{} this is a {} permission.",
            output::warn("Careful:"),
            output::risk(permission.risk())
        );
    }
    println!();

    workspace
        .ledger_mut()
        .allow(subject.clone(), permission.clone(), Actor::User, reason)?;
    record_permission(workspace.audit_mut(), &subject, &permission, true);
    workspace.save()?;

    println!(
        "{} {}",
        output::good("Allowed."),
        output::dim("Revoke it any time with `ephemeral revoke`.")
    );
    Ok(())
}

/// Revokes a permission.
pub(crate) fn revoke(home: &Path, reference: &str, permission: &str) -> Result<()> {
    let mut workspace = open(home)?;
    let subject = parse::principal(reference)?;
    let permission = parse::permission(&subject, permission)?;

    let revoked = workspace
        .ledger_mut()
        .revoke(&subject, &permission, Actor::User)?;

    if revoked == 0 {
        println!(
            "{}",
            output::dim(&format!(
                "{} was not allowed to {} — nothing to take back.",
                subject.label(),
                permission.describe()
            ))
        );
        return Ok(());
    }

    record_permission(workspace.audit_mut(), &subject, &permission, false);
    workspace.save()?;

    println!(
        "{} {} can no longer {}.",
        output::good("Revoked."),
        subject.label(),
        permission.describe()
    );
    if revoked > 1 {
        println!(
            "{}",
            output::dim(&format!(
                "{revoked} grants were withdrawn: revoking covers anything that would still \
                 have permitted this."
            ))
        );
    }
    Ok(())
}

fn record_permission(
    audit: &mut AuditLog,
    subject: &Principal,
    permission: &Permission,
    allowed: bool,
) {
    let event = if allowed {
        AuditEvent::PermissionDecided {
            principal: subject.clone(),
            permission: permission.clone(),
            decision: ephemeral_core::Decision::Allow,
        }
    } else {
        AuditEvent::PermissionRevoked {
            principal: subject.clone(),
            permission: permission.clone(),
        }
    };
    audit.append(Actor::User, event);
}

/// Puts an application away.
pub(crate) fn archive(home: &Path, reference: &str) -> Result<()> {
    transition(
        home,
        reference,
        LifecycleEvent::Archive,
        "archived from the command line",
        "Archived.",
    )
}

/// Brings an archived application back.
pub(crate) fn restore(home: &Path, reference: &str) -> Result<()> {
    transition(
        home,
        reference,
        LifecycleEvent::Restore,
        "restored from the command line",
        "Restored.",
    )
}

/// Deletes an application: withdraws every permission, moves its files aside.
pub(crate) fn delete(home: &Path, reference: &str) -> Result<()> {
    let mut workspace = open(home)?;
    let mut manifest = find(&workspace, reference)?;
    let id = manifest.id.clone();

    manifest.apply(TransitionRequest::new(
        LifecycleEvent::Delete,
        Actor::User,
        "deleted from the command line",
    ))?;

    // Capability goes immediately; data survives the recovery period.
    let revoked = workspace
        .ledger_mut()
        .revoke_all(&Principal::app(id.clone()));

    // The Deleted state *is* the tombstone: the record stays where it is so the
    // application can still be listed, inspected and restored during the
    // recovery period. Moving its files aside here would make "recoverable"
    // untrue — the manifest would go with them and nothing could find it again.
    // Purging is what destroys the tree (ADR-0009).
    workspace.apps_mut().save(&manifest)?;
    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppDeleted {
            app: id.clone(),
            grants_revoked: revoked,
        },
    );
    workspace.save()?;

    println!("{} {id}", output::good("Deleted."));
    println!(
        "{}",
        output::dim(&format!(
            "{revoked} permission(s) withdrawn. Its data is kept for {} — \
             `ephemeral purge {id}` destroys it now.",
            ephemeral_core::retention::DEFAULT_TRASH_PERIOD.describe()
        ))
    );
    Ok(())
}

/// Destroys an application and everything belonging to it.
pub(crate) fn purge(home: &Path, reference: &str, confirmed: bool) -> Result<()> {
    let workspace = open(home)?;
    let id = AppId::parse(reference)
        .with_context(|| format!("{reference:?} is not an application id"))?;

    if !confirmed {
        println!(
            "{} would destroy {id} and everything it holds — source, data, logs, artifacts.",
            output::warn("Purging")
        );
        println!("{}", output::dim("There is no way back from this."));
        println!();
        println!("Run it again with --yes if you mean it.");
        return Ok(());
    }

    let mut workspace = workspace;
    workspace
        .ledger_mut()
        .revoke_all(&Principal::app(id.clone()));
    workspace.apps().purge(&id)?;
    workspace.apps_mut().remove(&id).ok();
    workspace
        .audit_mut()
        .append(Actor::User, AuditEvent::AppPurged { app: id.clone() });
    workspace.save()?;

    println!(
        "{} {id} and all its data are gone.",
        output::good("Purged.")
    );
    Ok(())
}

fn transition(
    home: &Path,
    reference: &str,
    event: LifecycleEvent,
    reason: &str,
    success: &str,
) -> Result<()> {
    let mut workspace = open(home)?;
    let mut manifest = find(&workspace, reference)?;
    let before = manifest.lifecycle.state();

    let applied = manifest
        .apply(TransitionRequest::new(event, Actor::User, reason))
        .with_context(|| {
            format!(
                "cannot {event} an application that is {}",
                before.headline().to_lowercase()
            )
        })?;

    workspace.apps_mut().save(&manifest)?;
    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::LifecycleTransition {
            app: manifest.id.clone(),
            from: applied.from,
            to: applied.to,
            event,
            reason: reason.to_owned(),
        },
    );
    workspace.save()?;

    println!(
        "{} {} is now {}.",
        output::good(success),
        manifest.id,
        output::state(manifest.lifecycle.state())
    );
    println!("{}", output::dim(manifest.lifecycle.state().description()));
    Ok(())
}

/// Shows an application's lifecycle history.
pub(crate) fn logs(home: &Path, reference: &str) -> Result<()> {
    let workspace = open(home)?;
    let manifest = find(&workspace, reference)?;

    println!(
        "{}",
        output::heading(&format!("{} — history", manifest.name))
    );
    println!();

    if manifest.lifecycle.history().is_empty() {
        println!(
            "{}",
            output::dim("Nothing has happened yet. It was created and is waiting.")
        );
        return Ok(());
    }

    for entry in manifest.lifecycle.history() {
        println!(
            "{}  {} {}",
            output::dim(&entry.at.format("%Y-%m-%d %H:%M:%S").to_string()),
            output::state(entry.to),
            output::dim(&format!("({})", entry.from)),
        );
        println!("  {}", entry.explain());
        if let Some(error) = &entry.error {
            println!(
                "  {} {} — {}",
                output::bad("error:"),
                error.code,
                error.message
            );
        }
        for (key, value) in &entry.metadata {
            println!("  {}", output::dim(&format!("{key}: {value}")));
        }
    }

    Ok(())
}

/// Shows the security record.
pub(crate) fn audit(home: &Path, app: Option<&str>, limit: usize) -> Result<()> {
    let workspace = open(home)?;
    let log = workspace.audit();

    println!("{}", output::heading("Security record"));

    match log.verify() {
        Ok(()) => println!(
            "  {} {}",
            output::good("✓"),
            output::dim(&format!("{} entries, chain intact", log.len()))
        ),
        Err(error) => {
            println!("  {} {error}", output::bad("✗"));
            println!(
                "  {}",
                output::bad("The record has been altered. Treat this as a security event.")
            );
        }
    }
    println!();

    let entries: Vec<_> = match app {
        Some(reference) => {
            let id = AppId::parse(reference)
                .with_context(|| format!("{reference:?} is not an application id"))?;
            log.entries_for(&id)
        }
        None => log.entries().iter().collect(),
    };

    if entries.is_empty() {
        println!("{}", output::dim("Nothing recorded yet."));
        return Ok(());
    }

    for entry in entries.iter().rev().take(limit).rev() {
        println!("{}", entry.explain());
    }

    if entries.len() > limit {
        println!();
        println!(
            "{}",
            output::dim(&format!(
                "Showing the last {limit} of {}. Use --limit to see more.",
                entries.len()
            ))
        );
    }

    Ok(())
}

/// Shows the lifecycle state machine, or where one application sits in it.
pub(crate) fn states(home: &Path, app: Option<&str>) -> Result<()> {
    if let Some(reference) = app {
        let workspace = open(home)?;
        let manifest = find(&workspace, reference)?;
        let state = manifest.lifecycle.state();

        println!("{}", output::heading(&manifest.name));
        println!();
        println!(
            "  {}  {}",
            output::state(state),
            output::dim(state.as_str())
        );
        println!("  {}", state.description());
        println!();

        let available = manifest.lifecycle.available_events(Actor::User);
        if available.is_empty() {
            println!(
                "{}",
                output::dim("There is nothing you can do to it from here.")
            );
        } else {
            println!("{}", output::dim("What you can do from here"));
            for event in available {
                println!("  {:<20} {}", event.as_str(), output::dim(event.describe()));
            }
        }
        return Ok(());
    }

    println!("{}", output::heading("The application lifecycle"));
    println!();
    println!(
        "{}",
        output::dim(
            "Every application moves through this machine. Transitions are explicit: an \
             event that has no meaning in a state is refused, not ignored."
        )
    );
    println!();

    for state in LifecycleState::ALL {
        println!("{}  {}", output::state(state), output::dim(state.as_str()));
        println!("  {}", state.description());

        let outgoing = state.outgoing();
        if outgoing.is_empty() {
            println!("  {}", output::dim("nothing leads out of here"));
        } else {
            for (event, target) in outgoing {
                let destination = match target {
                    ephemeral_core::lifecycle::Target::State(next) => next.as_str().to_owned(),
                    ephemeral_core::lifecycle::Target::Resume => {
                        "back to where it was interrupted".to_owned()
                    }
                };
                let actors = event
                    .authorized_actors()
                    .iter()
                    .map(|actor| actor.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "  {:<22} → {:<24} {}",
                    event.as_str(),
                    destination,
                    output::dim(&format!("[{actors}]"))
                );
            }
        }
        println!();
    }

    Ok(())
}

fn is_put_away(state: LifecycleState) -> bool {
    matches!(state, LifecycleState::Archived | LifecycleState::Deleted)
}

fn parse_retention(value: &str) -> Result<RetentionPolicy> {
    Ok(match value {
        "one-shot" | "oneshot" => RetentionPolicy::OneShot,
        "ephemeral" => RetentionPolicy::Ephemeral {
            retain_for: RetentionPeriod::hours(24),
        },
        "temporary" => RetentionPolicy::Temporary {
            retain_for: RetentionPeriod::days(7),
        },
        "reusable" => RetentionPolicy::Reusable,
        "persistent" => RetentionPolicy::Persistent,
        other => bail!(
            "{other:?} is not a retention policy.\n\
             \n\
             Try one of:\n  \
             one-shot     created, run once, deleted\n  \
             ephemeral    kept for a day\n  \
             temporary    kept for a week\n  \
             reusable     kept until you archive it\n  \
             persistent   kept until you delete it"
        ),
    })
}

/// A readable name from what somebody typed, for when they did not give one.
fn derive_name(intent: &str) -> String {
    let words: Vec<&str> = intent
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .take(4)
        .collect();

    if words.is_empty() {
        return "App".to_owned();
    }

    let mut name = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            name.push(' ');
        }
        let cleaned: String = word
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect();
        let mut chars = cleaned.chars();
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.push_str(chars.as_str());
        }
    }
    name
}

/// The riskiest thing anything on this machine is currently allowed to do.
pub(crate) fn highest_granted_risk(workspace: &Workspace) -> Option<RiskLevel> {
    workspace
        .ledger()
        .grants()
        .iter()
        .filter(|grant| grant.decision.is_allowed() && !grant.is_revoked())
        .map(|grant| grant.permission.risk())
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_derived_from_what_somebody_typed() {
        assert_eq!(
            derive_name("compare these two CSV files and show me the differences"),
            "Compare These Two CSV"
        );
        assert_eq!(derive_name("build a todo list"), "Build A Todo List");
    }

    /// An intent with nothing usable in it still has to produce a name.
    #[test]
    fn a_name_is_produced_even_from_nonsense() {
        assert_eq!(derive_name("!!! ???"), "App");
        assert_eq!(derive_name(""), "App");
    }

    #[test]
    fn retention_policies_parse_and_typos_are_refused() {
        assert_eq!(
            parse_retention("one-shot").unwrap(),
            RetentionPolicy::OneShot
        );
        assert_eq!(
            parse_retention("reusable").unwrap(),
            RetentionPolicy::Reusable
        );
        assert!(parse_retention("temprary").is_err());

        let error = parse_retention("forever").unwrap_err().to_string();
        assert!(
            error.contains("persistent"),
            "the error should list what works"
        );
    }

    #[test]
    fn archived_and_deleted_applications_are_put_away() {
        assert!(is_put_away(LifecycleState::Archived));
        assert!(is_put_away(LifecycleState::Deleted));
        assert!(!is_put_away(LifecycleState::Ready));
        assert!(!is_put_away(LifecycleState::Running));
    }
}
