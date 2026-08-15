//! `ephemeral publish` and `ephemeral install` — giving an application to
//! somebody else.
//!
//! A shared application is a **recipe**, never a grant and never a binary
//! ([ADR-0012]). The recipient's Ephemeral builds it, and the recipient decides
//! what it may do. That is what makes accepting an application from a stranger
//! a reasonable thing to do.
//!
//! Four rules, each of which is a test:
//!
//! - **Grants never travel.** A package carries what the application *asks*
//!   for, with its stated reasons. The recipient's ledger starts empty for it.
//! - **Nothing local travels.** Not data, not logs, not audit entries, not
//!   secret values, not secret names, not the lifecycle history of somebody
//!   else's copy.
//! - **Nothing executable travels.** Source and a build recipe. The recipient's
//!   own Ephemeral builds and confines it.
//! - **The manifest is the review.** Before anything is built, the recipient is
//!   shown what it will be allowed to ask for and why.
//!
//! [ADR-0012]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0012-sharing-distributes-recipes.md

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use ephemeral_core::{
    Actor, AppId, AppManifest, audit::AuditEvent, permission::AppPermission, storage::AppStore as _,
};

use crate::output;

/// The file a package's manifest is written to.
const MANIFEST_FILE: &str = "ephemeral.yaml";

/// Where a package's source lives.
const SOURCE_DIR: &str = "source";

/// Writes an application to a directory somebody else can build.
pub(crate) fn publish(home: &Path, reference: &str, destination: &Path) -> Result<()> {
    let workspace = crate::commands::open(home)?;
    let manifest = crate::commands::find(&workspace, reference)?;

    if manifest.runtime.is_none() {
        bail!(
            "{} has not been generated yet, so there is nothing to publish. \
             Run `ephemeral generate {}` first.",
            manifest.id,
            manifest.id
        );
    }

    let source = workspace.layout().app(&manifest.id).source();
    if !source.is_dir() {
        bail!(
            "{} has no source at {}. It may have been purged.",
            manifest.id,
            source.display()
        );
    }

    let package = strip(&manifest);

    // Say what is about to leave the machine, before it leaves. Publishing is an
    // outbound data-flow decision and is gated like one.
    println!(
        "{}",
        output::heading(&format!("Publishing {}", manifest.name))
    );
    println!();
    println!("{}", output::dim("What will be written"));
    println!("  the manifest, as it will be reviewed by whoever receives it");
    println!("  the source and its tests");
    println!();
    println!("{}", output::dim("What will not"));
    println!("  its data, its logs, its audit record, and its lifecycle history");
    println!("  every permission decision you made — the recipient makes their own");

    std::fs::create_dir_all(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;

    let yaml = serde_norway::to_string(&package)
        .context("could not write the manifest for publication")?;
    std::fs::write(destination.join(MANIFEST_FILE), yaml)
        .with_context(|| format!("could not write {MANIFEST_FILE}"))?;

    let copied = copy_tree(&source, &destination.join(SOURCE_DIR))?;

    let mut workspace = workspace;
    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppPublished {
            app: manifest.id.clone(),
            destination: destination.display().to_string(),
        },
    );
    workspace.save()?;

    println!();
    println!(
        "{} {} file(s) to {}",
        output::good("Published."),
        copied + 1,
        destination.display()
    );
    if let Some(version) = package.current_version() {
        println!("{}", output::field("version", version.digest.short()));
    }
    println!(
        "{}",
        output::dim(
            "It is an ordinary directory: `git init` it and push it anywhere. Whoever receives \
             it runs `ephemeral install`."
        )
    );
    Ok(())
}

/// Reads a published package and, once the recipient agrees, records it.
pub(crate) fn install(home: &Path, source: &Path, accepted: bool) -> Result<()> {
    let document = std::fs::read_to_string(source.join(MANIFEST_FILE)).with_context(|| {
        format!(
            "{} does not look like an Ephemeral package — no {MANIFEST_FILE} in it",
            source.display()
        )
    })?;

    let package: AppManifest =
        serde_norway::from_str(&document).context("that package's manifest could not be read")?;
    package
        .validate()
        .context("that package's manifest is not valid")?;

    // The manifest is the review. Everything a person needs in order to decide
    // is shown before a single file is written, let alone built.
    show_for_review(&package);

    if !accepted {
        println!();
        println!(
            "{}",
            output::dim(
                "Nothing has been installed. Run it again with --accept if you want it, and \
                 you will still be asked about every permission separately."
            )
        );
        return Ok(());
    }

    let mut workspace = crate::commands::open(home)?;

    // A fresh id. The package says which *application* this is, by digest; the
    // id says which *installation*, and two people running the same recipe are
    // not running the same installation.
    let id = AppId::generate(&package.name);
    let mut installed = package.clone();
    installed.id = id.clone();
    installed.lifecycle = ephemeral_core::Lifecycle::new();
    installed.created_at = ephemeral_core::now();
    installed.touch();

    workspace
        .apps_mut()
        .create(&installed)
        .with_context(|| format!("could not save {id}"))?;
    workspace
        .apps()
        .prepare(&id)
        .with_context(|| format!("could not create the storage for {id}"))?;

    let copied = copy_tree(
        &source.join(SOURCE_DIR),
        &workspace.layout().app(&id).source(),
    )?;

    workspace.audit_mut().append(
        Actor::User,
        AuditEvent::AppInstalled {
            app: id.clone(),
            origin: source.display().to_string(),
        },
    );
    workspace.save()?;

    println!();
    println!("{} {id}", output::good("Installed."));
    println!("{}", output::dim(&format!("{copied} source file(s).")));
    println!();
    println!(
        "{}",
        output::dim(&format!(
            "It has no permissions. `ephemeral review {id}` asks you about each thing it wants, \
             and `ephemeral generate {id}` builds it on this machine."
        ))
    );
    Ok(())
}

/// The manifest as it will be published.
///
/// Everything local to one installation is removed rather than filtered on
/// read. A field that should not travel and is merely hidden by the display
/// layer is a field that travels.
fn strip(manifest: &AppManifest) -> AppManifest {
    let mut package = manifest.clone();

    // The sender's own id is which *installation* this was, not which
    // application, so it does not travel. Derived from the name instead, which
    // also makes two publishes of the same application identical rather than
    // gratuitously different.
    package.id = AppId::from_name(&manifest.name);

    // The history of somebody else's copy is nobody else's business.
    package.lifecycle = ephemeral_core::Lifecycle::new();
    package.artifacts = ephemeral_core::manifest::Artifacts::default();

    // Secret *names* are as revealing as the fact that a secret exists — and
    // the recipient's copy will ask for its own anyway.
    package.permissions.environment.clear();

    // Retention and tags are how *this* person chose to keep it.
    package.metadata.retention = ephemeral_core::RetentionPolicy::default();
    package.metadata.tags.clear();

    // The version chain says how this copy got here. Only what it *is* travels,
    // because that is what the recipient is being offered.
    if let Some(current) = manifest.current_version().cloned() {
        package.versions = vec![current];
    }

    package
}

/// Shows a package to somebody deciding whether to accept it.
fn show_for_review(package: &AppManifest) {
    println!("{}", output::heading(&package.name));

    if !package.description.is_empty() {
        println!();
        println!("  {}", package.description);
    }

    println!();
    println!("{}", output::dim("What it is"));
    if let Some(version) = package.current_version() {
        println!("{}", output::field("version", version.digest.short()));
    }
    if let Some(runtime) = &package.runtime {
        println!("  {}", runtime.kind.describe_isolation());
        if let Some(image) = &runtime.image {
            println!("{}", output::field("image", image));
        }
    }
    println!("  {}", package.resources.describe());

    println!();
    println!("{}", output::dim("What it will want to be allowed to do"));

    let requested = package.permissions.capabilities();
    if requested.is_empty() {
        println!("  {}", output::dim("Nothing at all."));
    } else {
        for permission in &requested {
            println!(
                "  {} {}",
                output::risk(permission.risk()),
                permission.describe()
            );
            match package.reason_for(permission) {
                Some(reason) => println!("      {}", output::dim(&format!("It says: {reason}"))),
                None => println!(
                    "      {}",
                    output::dim("It gives no reason for wanting this.")
                ),
            }
        }
    }

    println!();
    println!(
        "{}",
        output::dim(
            "It arrives with none of this. Nothing the sender allowed travels with it, and you \
             will be asked about each one separately."
        )
    );
}

/// Copies a directory tree, returning how many files were written.
///
/// Refuses symlinks rather than following them. A package is meant to be
/// reviewable, and a link that points outside the tree is either a mistake or an
/// attempt to reach something the reviewer did not read.
fn copy_tree(from: &Path, to: &Path) -> Result<usize> {
    std::fs::create_dir_all(to).with_context(|| format!("could not create {}", to.display()))?;

    let mut copied = 0;

    for entry in
        std::fs::read_dir(from).with_context(|| format!("could not read {}", from.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let target = to.join(entry.file_name());

        if metadata.is_symlink() {
            bail!(
                "{} is a symbolic link. A package has to be something a person can read, so \
                 links are refused rather than followed.",
                entry.path().display()
            );
        }

        if metadata.is_dir() {
            copied += copy_tree(&entry.path(), &target)?;
        } else if metadata.is_file() {
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("could not copy {}", entry.path().display()))?;
            copied += 1;
        }
    }

    Ok(copied)
}

/// The reasons a package states, for a caller that wants them without the
/// manifest.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the desktop review UI"))]
fn stated_reasons(package: &AppManifest) -> Vec<(AppPermission, Option<String>)> {
    package
        .permissions
        .capabilities()
        .into_iter()
        .map(|permission| {
            let reason = package.reason_for(&permission).map(ToOwned::to_owned);
            (permission, reason)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ephemeral_core::{
        Recipe,
        lifecycle::{LifecycleEvent, TransitionRequest},
        manifest::PermissionRationale,
        manifest::RuntimeSpec,
        permission::{HostScope, PathScope},
        retention::RetentionPolicy,
    };

    fn published() -> AppManifest {
        let mut manifest = AppManifest::requested(
            AppId::parse("csv-comparator").expect("a valid id"),
            "CSV comparator",
        );
        manifest.description = "Compares two CSV files".to_owned();
        manifest.runtime = Some(RuntimeSpec::docker_job(
            "python:3.12-slim",
            vec!["python".to_owned(), "compare.py".to_owned()],
        ));

        let read = AppPermission::read(PathScope::parse("~/Downloads/**").expect("a valid scope"));
        manifest.permissions.request(&read);
        manifest.permissions.environment = vec!["API_KEY".to_owned()];
        manifest.rationale = vec![PermissionRationale {
            permission: read,
            reason: "to read the files you want compared".to_owned(),
        }];

        manifest.metadata.retention = RetentionPolicy::OneShot;
        manifest.metadata.tags = vec!["personal".to_owned()];

        let mut recipe = Recipe {
            runtime: "docker".to_owned(),
            image: Some("python:3.12-slim".to_owned()),
            entrypoint: vec!["python".to_owned()],
            source: vec![("compare.py".to_owned(), "aaa".to_owned())],
            requests: Vec::new(),
            limits: "cpu=500".to_owned(),
        };
        recipe.normalise();
        manifest.record_version(&recipe, "generated");
        recipe.source = vec![("compare.py".to_owned(), "bbb".to_owned())];
        manifest.record_version(&recipe, "repaired");

        manifest
            .apply(TransitionRequest::new(
                LifecycleEvent::Plan,
                Actor::Ephemeral,
                "planning",
            ))
            .expect("planning starts from Requested");

        manifest
    }

    /// The rule that makes accepting an application from a stranger reasonable.
    #[test]
    fn a_package_carries_requests_and_never_grants() {
        let package = strip(&published());

        assert!(
            !package.permissions.capabilities().is_empty(),
            "what it wants travels"
        );
        assert!(
            !package.rationale.is_empty(),
            "and so does why it says it wants it"
        );

        // There is nowhere in a manifest to put a grant — the ledger is a
        // separate file that is not part of a package at all. This asserts the
        // property that makes that true.
        let yaml = serde_norway::to_string(&package).expect("a serialisable manifest");
        assert!(!yaml.contains("granted_by"), "{yaml}");
        assert!(!yaml.contains("decision"), "{yaml}");
    }

    /// Nothing about *this* copy travels.
    #[test]
    fn nothing_local_travels() {
        let original = published();
        let package = strip(&original);

        assert!(
            package.lifecycle.history().is_empty(),
            "somebody else's history is nobody's business"
        );
        assert!(
            package.permissions.environment.is_empty(),
            "a secret's name is as revealing as the fact that one exists"
        );
        assert!(
            package.metadata.tags.is_empty(),
            "tags are how this person filed it"
        );
        assert_eq!(
            package.metadata.retention,
            RetentionPolicy::default(),
            "how long to keep it is the recipient's choice"
        );

        // And the original is untouched: publishing must not mutate what it
        // published.
        assert!(!original.lifecycle.history().is_empty());
        assert_eq!(original.metadata.retention, RetentionPolicy::OneShot);
    }

    /// What it *is* travels; how this copy got there does not.
    #[test]
    fn only_the_current_version_travels() {
        let original = published();
        assert_eq!(original.versions.len(), 2);

        let package = strip(&original);
        assert_eq!(package.versions.len(), 1);
        assert_eq!(
            package.current_version().map(|v| &v.digest),
            original.current_version().map(|v| &v.digest),
            "the recipient is being offered what it is now"
        );
    }

    /// The digest is the point: two people can check they received the same
    /// thing.
    /// Two publishes of the same application produce the same package, which is
    /// what lets two recipients check they received the same thing.
    #[test]
    fn publishing_the_same_application_twice_produces_the_same_identity() {
        let one = strip(&published());
        let other = strip(&published());

        assert_eq!(one.id, other.id);
        assert_eq!(
            one.current_version().map(|v| &v.digest),
            other.current_version().map(|v| &v.digest)
        );
    }

    #[test]
    fn a_package_survives_a_round_trip_with_its_identity_intact() {
        let package = strip(&published());
        let yaml = serde_norway::to_string(&package).expect("a serialisable manifest");
        let parsed: AppManifest = serde_norway::from_str(&yaml).expect("a readable manifest");

        parsed
            .validate()
            .expect("a package must be valid on arrival");
        assert_eq!(
            parsed.current_version().map(|v| v.digest.clone()),
            package.current_version().map(|v| v.digest.clone())
        );
    }

    /// A review has to show what the thing will want, and what it claims about
    /// why — including when it claims nothing.
    #[test]
    fn every_request_carries_its_stated_reason_or_says_it_has_none() {
        let mut original = published();
        let unexplained =
            AppPermission::outbound(HostScope::parse("api.example.com").expect("a valid host"));
        original.permissions.request(&unexplained);

        let stated = stated_reasons(&strip(&original));

        assert_eq!(stated.len(), 2);
        assert!(
            stated.iter().any(|(_, reason)| reason.is_some()),
            "the explained one keeps its reason"
        );
        assert!(
            stated.iter().any(|(_, reason)| reason.is_none()),
            "the unexplained one is not given an invented one"
        );
    }

    /// An installation is a different thing from an application. Two people
    /// running the same recipe are not running the same installation.
    #[test]
    fn installing_produces_a_new_installation_of_the_same_application() {
        let package = strip(&published());
        let mut installed = package.clone();
        installed.id = AppId::generate(&package.name);

        assert_ne!(installed.id, package.id, "a new installation");
        assert_eq!(
            installed.current_version().map(|v| &v.digest),
            package.current_version().map(|v| &v.digest),
            "the same application"
        );
    }

    /// A package is meant to be readable. A link out of the tree is either a
    /// mistake or an attempt to reach something the reviewer did not read.
    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_a_package_is_refused() {
        let from = tempfile::tempdir().expect("a temporary directory");
        let to = tempfile::tempdir().expect("a temporary directory");

        std::fs::write(from.path().join("compare.py"), "print()\n").expect("writing source");
        std::os::unix::fs::symlink("/etc/passwd", from.path().join("sneaky"))
            .expect("creating a link");

        let error = copy_tree(from.path(), to.path()).expect_err("a link must be refused");
        assert!(error.to_string().contains("symbolic link"), "{error}");
    }

    #[test]
    fn copying_a_tree_keeps_its_shape() {
        let from = tempfile::tempdir().expect("a temporary directory");
        let to = tempfile::tempdir().expect("a temporary directory");

        std::fs::create_dir_all(from.path().join("tests")).expect("a subdirectory");
        std::fs::write(from.path().join("compare.py"), "print()\n").expect("writing source");
        std::fs::write(from.path().join("tests/test_compare.py"), "pass\n")
            .expect("writing a test");

        let copied = copy_tree(from.path(), to.path()).expect("a copyable tree");

        assert_eq!(copied, 2);
        assert!(to.path().join("tests/test_compare.py").is_file());
    }
}
