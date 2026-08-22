//! `ephemeral doctor` — check this machine and say what to do about it.
//!
//! A diagnostic that only reports symptoms wastes the user's time. Every check
//! here either passes quietly or says what is wrong *and* what would fix it.
//!
//! Absent Docker is a warning, not a failure. Ephemeral is designed to work
//! without it, and reporting its absence as an error would teach people to
//! ignore the output ([ADR-0005]).
//!
//! [ADR-0005]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0005-docker-first-runtime-abstraction.md

use std::path::Path;

use ephemeral_core::storage::Workspace;
use ephemeral_runtime::{Runtime as _, docker::DockerRuntime};

use crate::output;

/// How a check turned out.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Fine.
    Ok(String),
    /// Works, but the user should know something.
    Note(String, String),
    /// Broken, with what to do about it.
    Problem(String, String),
}

impl Verdict {
    fn print(&self) {
        match self {
            Self::Ok(what) => println!("{}", output::check(Some(true), what)),
            Self::Note(what, advice) => {
                println!("{}", output::check(None, what));
                println!("      {}", output::dim(advice));
            }
            Self::Problem(what, advice) => {
                println!("{}", output::check(Some(false), what));
                println!("      {}", output::dim(advice));
            }
        }
    }

    fn is_problem(&self) -> bool {
        matches!(self, Self::Problem(..))
    }
}

/// Runs every check.
pub(crate) fn run(home: &Path) {
    println!("{}", output::heading("Ephemeral doctor"));
    println!();

    let mut verdicts = Vec::new();

    println!("{}", output::dim("This machine"));
    verdicts.push(Verdict::Ok(format!(
        "{} on {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )));
    verdicts.push(storage(home));
    print_all(&verdicts);

    println!();
    println!("{}", output::dim("Container runtime"));
    let mut docker = vec![docker_check()];
    docker.extend(orphan_check(home));
    print_all(&docker);
    verdicts.append(&mut docker);

    println!();
    println!("{}", output::dim("What Ephemeral itself may do"));
    let mut authority = authority_checks(home);
    print_all(&authority);
    verdicts.append(&mut authority);

    println!();
    println!("{}", output::dim("Your applications"));
    let mut state = workspace_checks(home);
    print_all(&state);
    verdicts.append(&mut state);

    println!();
    let problems = verdicts.iter().filter(|v| v.is_problem()).count();
    if problems == 0 {
        println!("{}", output::good("Nothing wrong."));
    } else {
        println!(
            "{}",
            output::bad(&format!(
                "{problems} problem(s) to deal with, listed above."
            ))
        );
    }
}

fn print_all(verdicts: &[Verdict]) {
    for verdict in verdicts {
        verdict.print();
    }
}

/// Can Ephemeral actually write where it intends to?
fn storage(home: &Path) -> Verdict {
    let shown = home.display().to_string();

    if !home.exists() {
        return Verdict::Note(
            format!("storage at {shown} does not exist yet"),
            "That is fine on a fresh machine — it is created the first time you make an \
             application."
                .to_owned(),
        );
    }

    // Actually try it. A permissions bit that looks right and a directory you
    // cannot write to are different things, and only one of them is testable.
    let probe = home.join(".ephemeral-write-probe");
    match std::fs::write(&probe, b"probe") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Verdict::Ok(format!("storage at {shown} is writable"))
        }
        Err(error) => Verdict::Problem(
            format!("cannot write to {shown}: {error}"),
            "Check the directory's ownership and permissions, or point Ephemeral somewhere \
             else with EPHEMERAL_HOME."
                .to_owned(),
        ),
    }
}

/// What Ephemeral has been allowed to do, and what it is missing.
///
/// A diagnostic that reports the machine and not the permissions would answer
/// "Docker is available" to somebody whose `ephemeral run` is being refused
/// because Ephemeral was never allowed to use it. Both halves of that question
/// belong on the same page.
///
/// Nothing here is a failure. An empty ledger is what a new installation looks
/// like, and default deny is the design rather than a fault (ADR-0003) — so
/// these are notes with the command that resolves them.
fn authority_checks(home: &Path) -> Vec<Verdict> {
    let Ok(workspace) = Workspace::open(home) else {
        // Reported by the storage check; not this one's business.
        return Vec::new();
    };

    [
        (
            ephemeral_api::authority::RUNTIME,
            "build and run applications in containers",
        ),
        (
            ephemeral_api::authority::HOSTED_PROVIDER,
            "generate with a hosted model",
        ),
        (
            ephemeral_api::authority::CREDENTIAL,
            "use a model provider's credential",
        ),
    ]
    .into_iter()
    .map(|(permission, what_for)| {
        match ephemeral_api::authority::require(workspace.ledger(), &permission) {
            Ok(()) => Verdict::Ok(format!("may {what_for}")),
            Err(_) => Verdict::Note(
                format!("may not {what_for}"),
                ephemeral_api::authority::grant_argument(&permission).map_or_else(
                    || "Grant it from Ephemeral's own permissions.".to_owned(),
                    |written| format!("`ephemeral grant ephemeral {written}` allows it."),
                ),
            ),
        }
    })
    .collect()
}

/// Is there a container runtime, and is it usable?
///
/// Asks the runtime itself rather than probing Docker here. Two places deciding
/// whether Docker works would eventually disagree, and the one a user reads
/// would be the wrong one.
fn docker_check() -> Verdict {
    let availability = DockerRuntime::new().availability();

    if availability.usable {
        return Verdict::Ok(availability.explanation);
    }

    // A missing runtime is never a failure. Everything except running a
    // generated application works without it, and reporting its absence as an
    // error would teach people to ignore this output (ADR-0005).
    Verdict::Note(
        "no container runtime is available".to_owned(),
        availability.explanation,
    )
}

/// Is the container runtime holding anything no application accounts for?
///
/// Only asked when the runtime is usable and the workspace is readable —
/// neither is a finding of this check, and both are reported elsewhere.
fn orphan_check(home: &Path) -> Option<Verdict> {
    let runtime = DockerRuntime::new();
    if !runtime.availability().usable {
        return None;
    }

    let workspace = Workspace::open(home).ok()?;
    let orphans = crate::runtime::orphans(&workspace, &runtime).ok()?;

    if orphans.is_empty() {
        return Some(Verdict::Ok(
            "no containers left over from anything".to_owned(),
        ));
    }

    // A leftover container holds disk and a name, and may still have a mount of
    // the user's files. That makes it worth saying out loud rather than
    // reaping silently on somebody's behalf.
    Some(Verdict::Note(
        format!("{} container(s) no application accounts for", orphans.len()),
        format!(
            "{} — `ephemeral cleanup` lists them, `ephemeral cleanup --yes` removes them.",
            orphans
                .iter()
                .map(|orphan| orphan.container.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

/// Is what is on disk consistent and trustworthy?
fn workspace_checks(home: &Path) -> Vec<Verdict> {
    let workspace = match Workspace::open(home) {
        Ok(workspace) => workspace,
        Err(error) => {
            return vec![Verdict::Problem(
                format!("Ephemeral's files could not be read: {error}"),
                "If a file is corrupt, move it aside and Ephemeral will start fresh — but \
                 you will lose the permission decisions it held."
                    .to_owned(),
            )];
        }
    };

    let mut verdicts = Vec::new();

    match workspace.load_all() {
        Ok(result) => {
            verdicts.push(Verdict::Ok(format!(
                "{} application(s) readable",
                result.loaded.len()
            )));
            for (id, problem) in result.broken {
                verdicts.push(Verdict::Problem(
                    format!("{id} cannot be read: {problem}"),
                    format!(
                        "Inspect {}, or purge it with `ephemeral purge {id} --yes`.",
                        workspace.layout().app(&id).manifest().display()
                    ),
                ));
            }
        }
        Err(error) => verdicts.push(Verdict::Problem(
            format!("applications could not be listed: {error}"),
            "Check that the applications directory is readable.".to_owned(),
        )),
    }

    // The audit log is a security control, so a failure here is a security
    // event rather than a warning in a file nobody reads.
    match workspace.audit().verify() {
        Ok(()) => verdicts.push(Verdict::Ok(format!(
            "audit record intact ({} entries)",
            workspace.audit().len()
        ))),
        Err(error) => verdicts.push(Verdict::Problem(
            format!("the audit record has been altered: {error}"),
            "Something changed a security record that is only ever appended to. Treat this \
             as a security event, not a corruption."
                .to_owned(),
        )),
    }

    let granted = workspace.ledger().grants().len();
    verdicts.push(Verdict::Ok(format!(
        "{granted} permission decision(s) on record"
    )));

    if let Some(risk) = crate::commands::highest_granted_risk(&workspace) {
        if risk.requires_explicit_confirmation() {
            verdicts.push(Verdict::Note(
                format!("something here holds a {} permission", output::risk(risk)),
                "Review it with `ephemeral permissions <app>` — and `ephemeral permissions \
                 ephemeral` for the product's own."
                    .to_owned(),
            ));
        }
    }

    verdicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_missing_directory_is_a_note_rather_than_a_problem() {
        let directory = TempDir::new().unwrap();
        let verdict = storage(&directory.path().join("not-yet"));

        assert!(!verdict.is_problem(), "a fresh machine is not broken");
        assert!(matches!(verdict, Verdict::Note(..)));
    }

    #[test]
    fn a_writable_directory_passes() {
        let directory = TempDir::new().unwrap();
        let verdict = storage(directory.path());

        assert!(matches!(verdict, Verdict::Ok(_)), "{verdict:?}");
        assert!(
            !directory.path().join(".ephemeral-write-probe").exists(),
            "the probe must clean up after itself"
        );
    }

    /// Absence of Docker must never be a failure: Ephemeral works without it,
    /// and crying wolf teaches people to ignore the output.
    #[test]
    fn a_missing_container_runtime_is_never_a_problem() {
        assert!(!docker_check().is_problem());
    }

    #[test]
    fn a_fresh_workspace_reports_no_problems() {
        let directory = TempDir::new().unwrap();
        let verdicts = workspace_checks(directory.path());

        assert!(
            verdicts.iter().all(|v| !v.is_problem()),
            "a fresh machine should be clean: {verdicts:?}"
        );
    }

    /// A tampered audit record must be reported as a security event, because
    /// that is what it is.
    #[test]
    fn an_altered_audit_record_is_reported_as_a_problem() {
        let directory = TempDir::new().unwrap();
        std::fs::create_dir_all(directory.path()).unwrap();

        {
            use ephemeral_core::{Actor, AppId, audit::AuditEvent};
            let mut workspace = Workspace::open(directory.path()).unwrap();
            workspace.audit_mut().append(
                Actor::User,
                AuditEvent::AppPurged {
                    app: AppId::parse("gone").unwrap(),
                },
            );
            workspace.save().unwrap();
        }

        let path = directory.path().join(ephemeral_core::storage::AUDIT_FILE);
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, written.replace("\"gone\"", "\"other\"")).unwrap();

        let verdicts = workspace_checks(directory.path());
        assert!(
            verdicts.iter().any(Verdict::is_problem),
            "a rewritten record must be reported: {verdicts:?}"
        );
    }
}
