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

/// `ephemeral models` — check the connection, and see what can be asked for.
///
/// One command for two questions, because they have one answer and the same
/// failure. "Is my key right" and "does this model exist" are both settled by
/// asking the service what it has; asking them separately gives two ways to be
/// almost-configured, and the second one is discovered after the first token
/// has been paid for.
pub(crate) fn models(home: &Path, provider_name: &str) -> Result<()> {
    let workspace = crate::commands::open(home)?;

    println!("{} {provider_name}", output::dim("Asking"));

    let listed = ephemeral_engine::models(&workspace, provider_name)?;

    println!();
    if listed.is_empty() {
        // Reached, and it said nothing. Worth distinguishing from a refusal:
        // the credential works and the connection works, and there is still
        // nothing to choose from.
        println!(
            "{}",
            output::warn("It answered, and listed no models it can be asked for.")
        );
        return Ok(());
    }

    println!(
        "{}",
        output::good(&format!(
            "Reached {provider_name}. {} model(s).",
            listed.len()
        ))
    );
    println!();

    for model in &listed {
        // The id is what goes in a request and the name is what a person reads,
        // and they are often the same string. Printing both when they are would
        // be noise.
        let label = if model.name == model.id {
            String::new()
        } else {
            model.name.clone()
        };

        // The ceiling, where the service publishes one, because it is the
        // setting most likely to be wrong: a request for a larger reply than
        // the model can hold is refused outright, with a message about a field
        // nobody typed.
        let ceiling = model
            .ceiling
            .map(|tokens| format!("up to {tokens} tokens"))
            .unwrap_or_default();

        println!(
            "  {:<40} {}",
            model.id,
            output::dim([label, ceiling].join("  ").trim())
        );
    }

    println!();
    for line in advice(provider_name, &listed) {
        println!("{}", output::dim(&line));
    }

    Ok(())
}

/// What to do with the list, for the provider in question.
fn advice(provider_name: &str, listed: &[ephemeral_agent::Model]) -> Vec<String> {
    let mut lines = vec![match provider_name {
        "anthropic" => format!(
            "Set {} to one of these.",
            ephemeral_provider_anthropic::MODEL_VARIABLE
        ),
        "openai" => format!(
            "Set {} to one of these.",
            ephemeral_provider_openai::MODEL_VARIABLE
        ),
        "local" => format!(
            "Set {} to one of these.",
            ephemeral_provider_local::MODEL_VARIABLE
        ),
        _ => "This provider takes no model name.".to_owned(),
    }];

    // Said once, here, rather than discovered as a refusal after a model was
    // chosen. Every model this small was unusable until the ceiling became a
    // setting, and nothing in the output explained why.
    let ceiling_variable = match provider_name {
        "anthropic" => Some(ephemeral_provider_anthropic::MAX_TOKENS_VARIABLE),
        "openai" => Some(ephemeral_provider_openai::MAX_TOKENS_VARIABLE),
        "local" => Some(ephemeral_provider_local::MAX_TOKENS_VARIABLE),
        _ => None,
    };

    if let Some(variable) = ceiling_variable
        && listed.iter().any(|model| {
            model
                .ceiling
                .is_some_and(|tokens| tokens < ephemeral_provider_openai::wire::DEFAULT_MAX_TOKENS)
        })
    {
        lines.push(format!(
            "Some of these hold less than Ephemeral asks for by default ({} tokens). \
             For one of those, set {variable} to its ceiling or below, or the service \
             refuses the request.",
            ephemeral_provider_openai::wire::DEFAULT_MAX_TOKENS
        ));
    }

    lines
}
