//! The desktop window's Rust half.
//!
//! Every command here does the same three things: open the workspace, ask
//! `ephemeral-api` for a view, hand it over. Nothing in this file evaluates a
//! permission, computes a lifecycle transition, or joins a path — a client that
//! did any of those would be a second, subtly different Ephemeral, which is the
//! failure this layer exists to prevent.
//!
//! It is deliberately thin. The interesting decisions are in `ephemeral-core`
//! and the interesting rendering is in `ui/render.js`, and both are tested
//! without a window. What is left is the part a compiler checks.

use std::path::PathBuf;

use ephemeral_api::{ApplicationDetail, ApplicationSummary, AuditEntryView};
use ephemeral_core::{AppId, storage::Workspace};

/// Where Ephemeral keeps its state, unless told otherwise.
const HOME_VARIABLE: &str = "EPHEMERAL_HOME";

/// Anything a command can fail with, phrased for a person.
///
/// A string rather than a typed error because it crosses into JavaScript, and
/// the messages the core produces are already written to be read. Inventing a
/// second vocabulary on this side is how the window ends up explaining things
/// differently from the terminal.
type Failure = String;

/// Opens the workspace, saying where it looked if that fails.
fn open() -> Result<Workspace, Failure> {
    let home = home_directory()?;

    Workspace::open(&home).map_err(|error| {
        format!("Could not open Ephemeral's files at {}: {error}", home.display())
    })
}

/// Where Ephemeral's state lives.
///
/// The same rules the CLI uses, because two clients disagreeing about where an
/// application lives would be worse than either being wrong.
fn home_directory() -> Result<PathBuf, Failure> {
    if let Some(explicit) = std::env::var_os(HOME_VARIABLE) {
        return Ok(PathBuf::from(explicit));
    }

    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or_else(|| "Could not work out where your home directory is.".to_owned())?;

    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("ephemeral"))
}

/// Every application, most recently touched first.
#[tauri::command]
fn applications() -> Result<Vec<ApplicationSummary>, Failure> {
    let workspace = open()?;
    let loaded = workspace
        .load_all()
        .map_err(|error| format!("Could not read your applications: {error}"))?;

    Ok(ephemeral_api::applications(&loaded.loaded, workspace.ledger()))
}

/// One application's page.
#[tauri::command]
fn application(id: String) -> Result<ApplicationDetail, Failure> {
    let workspace = open()?;
    let app = AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;

    let manifest = workspace
        .apps()
        .load(&app)
        .map_err(|_| format!("There is no application called {id}."))?;

    Ok(ephemeral_api::application(&manifest, &workspace))
}

/// The security record, newest first.
#[tauri::command]
fn activity(limit: usize) -> Result<Vec<AuditEntryView>, Failure> {
    let workspace = open()?;

    Ok(ephemeral_api::recent_activity(workspace.audit(), None, limit))
}

/// Which view shape this window speaks.
///
/// Exposed so the window can refuse to run against a service it does not
/// understand rather than misreading one.
#[tauri::command]
const fn api_version() -> u32 {
    ephemeral_api::API_VERSION
}

/// Starts the window.
///
/// # Panics
///
/// If Tauri cannot create a window at all, which is not a condition this
/// application can do anything useful about.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            applications,
            application,
            activity,
            api_version
        ])
        .run(tauri::generate_context!())
        .expect("the desktop window could not be created");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two clients disagreeing about where an application lives would be worse
    /// than either being wrong.
    #[test]
    fn an_explicit_home_is_honoured() {
        // SAFETY: single-threaded test, and the variable is read immediately.
        unsafe { std::env::set_var(HOME_VARIABLE, "/tmp/ephemeral-desktop-test") };

        assert_eq!(
            home_directory().expect("an explicit home"),
            PathBuf::from("/tmp/ephemeral-desktop-test")
        );

        unsafe { std::env::remove_var(HOME_VARIABLE) };
    }

    /// The window speaks a version, so it can refuse a service it does not
    /// understand instead of misreading one.
    #[test]
    fn the_window_reports_the_api_it_speaks() {
        assert_eq!(api_version(), ephemeral_api::API_VERSION);
    }
}
