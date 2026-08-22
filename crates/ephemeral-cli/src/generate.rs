//! `ephemeral generate` — the terminal's half of turning a description into an
//! application.
//!
//! The run itself — plan, write, build, test, repair, record — is
//! [`ephemeral_engine::generate`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-engine),
//! which the desktop window calls too. What is left here is choosing which
//! application somebody meant, and saying what happened afterwards.

use std::path::Path;

use anyhow::Result;

use crate::output;

/// Builds an application from the intent already recorded for it.
pub(crate) fn run(home: &Path, reference: &str, provider_name: &str) -> Result<()> {
    let mut workspace = crate::commands::open(home)?;
    let mut manifest = crate::commands::find(&workspace, reference)?;

    println!(
        "{} {} with {provider_name}",
        output::dim("Generating"),
        manifest.id
    );

    let built = ephemeral_engine::generate(&mut workspace, &mut manifest, provider_name)?;

    for warning in &built.warnings {
        eprintln!("{} {warning}", output::warn("warning"));
    }

    println!();
    println!("{}", output::good(&built.headline));
    println!("{}", output::dim(&built.how_it_went));
    if let Some(version) = &built.version {
        println!("{}", output::field("version", version));
    }

    println!();
    println!("{}", output::dim("What it will ask for"));
    if built.requests.is_empty() {
        println!(
            "  {}",
            output::dim("Nothing. It can run with no access at all.")
        );
    } else {
        for request in &built.requests {
            println!("  {} {}", output::risk_named(&request.risk), request.wants);
        }
        println!();
        println!(
            "{}",
            output::dim(&format!(
                "It has none of these yet. `ephemeral permissions {}` shows what it wants; \
                 `ephemeral grant` is how it gets any of it.",
                manifest.id
            ))
        );
    }

    if let Some(widened) = &built.widened {
        println!();
        println!("{} {widened}", output::warn("This update wants more."));
        if built.grants_withdrawn > 0 {
            println!(
                "{}",
                output::dim(&format!(
                    "{} permission(s) you had allowed were withdrawn, because they no longer \
                     cover what it now asks for.",
                    built.grants_withdrawn
                ))
            );
        }
        println!(
            "{}",
            output::dim(&format!(
                "Run `ephemeral review {}` to decide. Until you do, it has less than it had.",
                manifest.id
            ))
        );
    } else if let Some(unchanged) = &built.unchanged {
        println!();
        println!("{}", output::dim(unchanged));
    }

    Ok(())
}
