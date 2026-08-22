//! The desktop window's Rust half.
//!
//! Every command here does the same three things: open the workspace, ask
//! `ephemeral-api` for a view or an operation, hand the result over. Nothing in
//! this file evaluates a permission, computes a lifecycle transition, joins a
//! path, or arranges the steps of an operation itself — a client that did any of
//! those would be a second, subtly different Ephemeral, which is the failure
//! this layer exists to prevent.
//!
//! It is deliberately thin. The interesting decisions are in `ephemeral-core`
//! and the interesting rendering is in `ui/render.js`, and both are tested
//! without a window. What is left is the part a compiler checks.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use ephemeral_api::{ApplicationDetail, ApplicationSummary, AuditEntryView, HistoryView};
use ephemeral_core::{
    AppId,
    lifecycle::LifecycleEvent,
    storage::{AppStore as _, Workspace},
};

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


/// What starting an application told the window.
#[derive(serde::Serialize)]
struct Run {
    state: String,
    container: Option<String>,
    confinement: Vec<String>,
    refused: Vec<String>,
    inert: Option<String>,
}

/// What an application has been, and what it has printed.
#[derive(serde::Serialize)]
struct Logs {
    history: Vec<HistoryView>,
    output: Option<String>,
}

/// A generation run, as a window reads it.
#[derive(serde::Serialize)]
struct GenerationView {
    running: bool,
    built: Option<Built>,
    failed: Option<String>,
}

/// What a finished run produced.
///
/// The engine's own type, restated for the wire: it crosses into JavaScript, so
/// it has to serialise, and the engine has no business knowing that.
#[derive(serde::Serialize)]
struct Built {
    headline: String,
    how_it_went: String,
    version: Option<String>,
    requests: Vec<Requested>,
    widened: Option<String>,
    grants_withdrawn: usize,
    unchanged: Option<String>,
    warnings: Vec<String>,
}

/// One capability a generated application will ask for.
#[derive(serde::Serialize)]
struct Requested {
    wants: String,
    risk: String,
}

impl From<ephemeral_engine::Generated> for Built {
    fn from(built: ephemeral_engine::Generated) -> Self {
        Self {
            headline: built.headline,
            how_it_went: built.how_it_went,
            version: built.version,
            requests: built
                .requests
                .into_iter()
                .map(|request| Requested {
                    wants: request.wants,
                    risk: request.risk,
                })
                .collect(),
            widened: built.widened,
            grants_withdrawn: built.grants_withdrawn,
            unchanged: built.unchanged,
            warnings: built.warnings,
        }
    }
}

/// A generation run in flight, or its result waiting to be read.
enum Generation {
    Running,
    Built(Box<ephemeral_engine::Generated>),
    Failed(String),
}

/// What the window says when its own bookkeeping has broken.
const RUNS_POISONED: &str = "This window lost track of a generation run. Reopen it, and \
                             `ephemeral inspect` will say where the application got to.";

/// Runs in flight, and results not yet read.
///
/// Deliberately in the window rather than on disk: a generation that was
/// interrupted leaves its state in the *manifest*, which is the record that
/// matters, and this is only what the page shows while it happens.
fn generation_state() -> &'static Mutex<HashMap<String, Generation>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Generation>>> = OnceLock::new();

    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An application id, or a refusal phrased for a person.
fn parse_id(id: &str) -> Result<AppId, Failure> {
    AppId::parse(id).map_err(|error| format!("{id} is not an application id: {error}"))
}

/// One application's manifest.
fn load(workspace: &Workspace, id: &str) -> Result<ephemeral_core::AppManifest, Failure> {
    let app = parse_id(id)?;

    workspace
        .apps()
        .load(&app)
        .map_err(|_| format!("There is no application called {id}."))
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

/// Records a new application from what somebody typed into the window.
///
/// The whole operation is `ephemeral-api`'s, so the window creates applications
/// the same way the terminal and a phone do rather than a fourth similar way.
/// Nothing is generated, built or run: this is the act of asking, and the
/// application lands in the state the lifecycle calls *requested*.
#[tauri::command]
fn create(intent: String) -> Result<ApplicationSummary, Failure> {
    let mut workspace = open()?;

    let manifest = ephemeral_api::create(
        &mut workspace,
        &intent,
        None,
        ephemeral_core::retention::RetentionPolicy::default(),
    )?;

    Ok(ApplicationSummary::of(&manifest, workspace.ledger()))
}

/// The security record, newest first.
#[tauri::command]
fn activity(limit: usize) -> Result<Vec<AuditEntryView>, Failure> {
    let workspace = open()?;

    Ok(ephemeral_api::recent_activity(workspace.audit(), None, limit))
}

/// Records a person's decision about one thing an application asked for.
///
/// The window is the *asking* half and nothing more. Whether a decision is
/// permitted, what it covers, and what it means are all decided by the ledger
/// in `ephemeral-core` — this only carries an answer a human gave.
///
/// `allow` is not defaulted anywhere, and there is no bulk variant. A window
/// that could grant several things at once would be a window that grants things
/// nobody read.
#[tauri::command]
fn decide(id: String, capability: String, target: Option<String>, allow: bool) -> Result<(), Failure> {
    let mut workspace = open()?;
    let app = AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;

    let manifest = workspace
        .apps()
        .load(&app)
        .map_err(|_| format!("There is no application called {id}."))?;

    // Matched against what the application actually asked for, rather than
    // rebuilt from strings the window sent. A client that could compose a
    // permission out of a capability name and a path would be a client that can
    // grant something nobody requested.
    let permission = manifest
        .permissions
        .capabilities()
        .into_iter()
        .find(|permission| {
            permission.capability() == capability
                && target.as_ref().is_none_or(|wanted| &permission.describe() == wanted)
        })
        .ok_or_else(|| format!("{id} has not asked for {capability}."))?;

    let subject = ephemeral_core::Principal::app(app.clone());
    let decided = ephemeral_core::permission::Permission::App(permission.clone());
    let reason = manifest
        .reason_for(&permission)
        .unwrap_or("no reason given")
        .to_owned();

    let ledger = workspace.ledger_mut();
    let recorded = if allow {
        ledger.allow(subject.clone(), decided.clone(), ephemeral_core::Actor::User, reason)
    } else {
        ledger.deny(
            subject.clone(),
            decided.clone(),
            ephemeral_core::Actor::User,
            "declined in the desktop window",
        )
    };
    recorded.map_err(|error| format!("That decision could not be recorded: {error}"))?;

    workspace.audit_mut().append(
        ephemeral_core::Actor::User,
        ephemeral_core::audit::AuditEvent::PermissionDecided {
            principal: subject,
            permission: decided,
            decision: if allow {
                ephemeral_core::permission::Decision::Allow
            } else {
                ephemeral_core::permission::Decision::Deny
            },
        },
    );

    workspace
        .save()
        .map_err(|error| format!("That decision could not be saved: {error}"))
}

/// Returns an application to a version it used to be.
///
/// The whole operation is `ephemeral-api`'s: the source on disk going back, the
/// manifest recording it, the built image being cleared, and the withdrawal of
/// any grant the older version would otherwise inherit. Those four are one act,
/// and a window that sequenced them itself would eventually sequence them
/// differently from the terminal — with the difference showing up as an
/// application holding a permission nobody approved for the code it now runs.
///
/// `version` is a digest, matched by prefix against what this application
/// actually recorded. The window sends one it was given rather than one it
/// composed, but the matching is the service layer's either way: a digest that
/// is not in the history is not a version of this application.
#[tauri::command]
fn rollback(id: String, version: String) -> Result<ephemeral_api::Rollback, Failure> {
    let mut workspace = open()?;
    let app =
        AppId::parse(&id).map_err(|error| format!("{id} is not an application id: {error}"))?;

    ephemeral_api::rollback(&mut workspace, &app, &version)
}


/// Starts an application in its sandbox.
///
/// The whole operation is the engine's — the same one the terminal calls — so
/// what a window starts is confined exactly as what a terminal starts. The
/// arguments are the application's own, and are passed as typed: what they
/// mean is the application's business, and a window that interpreted them
/// would be inventing behaviour the terminal does not have.
#[tauri::command]
fn start(id: String, arguments: Vec<String>) -> Result<Run, Failure> {
    let mut workspace = open()?;
    let mut manifest = load(&workspace, &id)?;

    let started = ephemeral_engine::container::start(
        &mut workspace,
        &mut manifest,
        &arguments,
        "started from the desktop window",
    )
    .map_err(|error| format!("{error:#}"))?;

    Ok(Run {
        state: manifest.lifecycle.state().headline().to_owned(),
        container: started.container,
        confinement: started.confinement,
        refused: started.refused,
        inert: started.inert,
    })
}

/// Stops a running application.
#[tauri::command]
fn halt(id: String) -> Result<(), Failure> {
    let mut workspace = open()?;
    let mut manifest = load(&workspace, &id)?;

    ephemeral_engine::container::stop(
        &mut workspace,
        &mut manifest,
        "stopped from the desktop window",
    )
    .map_err(|error| format!("{error:#}"))
}

/// Brings an application's record back in line with its container.
///
/// A window can ask this on a timer, which is the closest thing it has to the
/// terminal's `watch`: a crash nobody is looking at is noticed the next time
/// anything asks, and this is what asks.
#[tauri::command]
fn refresh(id: String) -> Result<String, Failure> {
    let mut workspace = open()?;
    let mut manifest = load(&workspace, &id)?;

    let reconciled = ephemeral_engine::container::reconcile(&mut workspace, &mut manifest)
        .map_err(|error| format!("{error:#}"))?;

    Ok(reconciled
        .because
        .unwrap_or_else(|| manifest.lifecycle.state().headline().to_owned()))
}

/// What an application has been, and what it has printed.
#[tauri::command]
fn logs(id: String, lines: u32) -> Result<Logs, Failure> {
    let workspace = open()?;
    let manifest = load(&workspace, &id)?;

    Ok(Logs {
        history: ephemeral_api::history(&manifest),
        output: ephemeral_engine::container::output(&manifest, lines),
    })
}

/// Puts an application away, brings it back, or throws it away.
///
/// One command for the three, because they are one operation in the service
/// layer — and because a window with three near-identical commands is a window
/// where two of them drift.
#[tauri::command]
fn move_app(id: String, event: String) -> Result<ephemeral_api::Moved, Failure> {
    let mut workspace = open()?;
    let app = parse_id(&id)?;

    // Matched against a fixed set rather than parsed from what was sent: a
    // client that could name any lifecycle event could drive an application
    // into a state nobody asked for.
    let event = match event.as_str() {
        "archive" => LifecycleEvent::Archive,
        "restore" => LifecycleEvent::Restore,
        "delete" => LifecycleEvent::Delete,
        other => return Err(format!("{other} is not something this window can do.")),
    };

    ephemeral_api::move_to(
        &mut workspace,
        &app,
        event,
        "decided in the desktop window",
    )
}

/// Destroys an application and everything it holds.
#[tauri::command]
fn purge(id: String) -> Result<ephemeral_api::Moved, Failure> {
    let mut workspace = open()?;
    let app = parse_id(&id)?;

    ephemeral_api::purge(&mut workspace, &app)
}

/// Brings every application's record back in line with its container.
///
/// The terminal has `watch` for this, and a window has something better: it is
/// already redrawing. Without it the list is a record that lies by omission —
/// an application that crashed while nobody was looking still reads as running,
/// which is exactly what a person opening the window is trying to find out.
///
/// Quiet about being unable to look. If Ephemeral may not drive a container
/// runtime, or there is none, there is nothing to reconcile against and nothing
/// worth interrupting somebody to say: the states on screen are simply the last
/// ones recorded.
#[tauri::command]
fn sweep() -> Vec<String> {
    let Ok(mut workspace) = open() else {
        return Vec::new();
    };
    let Ok(runtime) = ephemeral_engine::sandbox::usable_runtime(&workspace) else {
        return Vec::new();
    };

    ephemeral_engine::container::sweep(&mut workspace, &runtime)
        .map(|acted| {
            acted
                .into_iter()
                .map(|action| format!("{}: {}", action.app, action.what))
                .collect()
        })
        .unwrap_or_default()
}

/// What Ephemeral itself may do, and what it may not.
#[tauri::command]
fn authority() -> Result<Vec<ephemeral_api::authority::AuthorityView>, Failure> {
    let workspace = open()?;

    Ok(ephemeral_api::authority::overview(workspace.ledger()))
}

/// Records a decision about something Ephemeral itself may do.
///
/// The most powerful consent in the product: this authority outlives every
/// application and covers all of them at once. The capability is matched
/// against what the service layer offers, never composed from what the window
/// sent.
#[tauri::command]
fn decide_authority(capability: String, allow: bool) -> Result<(), Failure> {
    let mut workspace = open()?;

    ephemeral_api::authority::decide(&mut workspace, &capability, allow)
}

/// Starts generating, and returns immediately.
///
/// Generation takes minutes, and a command that blocked for minutes would
/// freeze the window. So it runs on a thread and writes what it is doing where
/// [`generation`] can read it — which is also what lets somebody close the page
/// and come back to a finished application.
#[tauri::command]
fn generate(id: String, provider: String) -> Result<(), Failure> {
    let app = parse_id(&id)?;

    {
        let mut runs = generation_state().lock().map_err(|_| RUNS_POISONED)?;
        if matches!(runs.get(&id), Some(Generation::Running)) {
            return Err(format!("{id} is already being generated."));
        }
        runs.insert(id.clone(), Generation::Running);
    }

    std::thread::spawn(move || {
        let finished = generate_now(&app, &provider);

        if let Ok(mut runs) = generation_state().lock() {
            runs.insert(id, finished);
        }
    });

    Ok(())
}

/// One generation run, start to finish, on the thread that owns it.
fn generate_now(app: &AppId, provider: &str) -> Generation {
    let mut workspace = match open() {
        Ok(workspace) => workspace,
        Err(error) => return Generation::Failed(error),
    };
    let mut manifest = match workspace.apps().load(app) {
        Ok(manifest) => manifest,
        Err(_) => return Generation::Failed(format!("There is no application called {app}.")),
    };

    match ephemeral_engine::generate(&mut workspace, &mut manifest, provider) {
        Ok(built) => Generation::Built(Box::new(built)),
        // `{error:#}` rather than `{error}`: the useful half of a generation
        // failure is usually the cause underneath it — "the model said the API
        // key is invalid", not "could not generate".
        Err(error) => Generation::Failed(format!("{error:#}")),
    }
}

/// How a generation run is going, or how it went.
///
/// Polled rather than pushed. Progress arrives in the application's own
/// lifecycle — planning, writing, building, testing — which is already saved to
/// disk as it happens, so a window that re-reads the application is watching
/// the real thing rather than a second account of it.
#[tauri::command]
fn generation(id: String) -> Result<Option<GenerationView>, Failure> {
    let runs = generation_state().lock().map_err(|_| RUNS_POISONED)?;

    Ok(runs.get(&id).map(|state| match state {
        Generation::Running => GenerationView {
            running: true,
            built: None,
            failed: None,
        },
        Generation::Built(built) => GenerationView {
            running: false,
            built: Some(built.as_ref().clone().into()),
            failed: None,
        },
        Generation::Failed(why) => GenerationView {
            running: false,
            built: None,
            failed: Some(why.clone()),
        },
    }))
}

/// Forgets a finished run, so the page stops reporting it.
#[tauri::command]
fn acknowledge(id: String) -> Result<(), Failure> {
    let mut runs = generation_state().lock().map_err(|_| RUNS_POISONED)?;
    if !matches!(runs.get(&id), Some(Generation::Running)) {
        runs.remove(&id);
    }

    Ok(())
}

/// Which providers this window can offer.
#[tauri::command]
fn providers() -> Vec<String> {
    ephemeral_engine::PROVIDERS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// What this machine can and cannot do, and what to do about it.
#[tauri::command]
fn diagnostics() -> Result<Vec<ephemeral_engine::Check>, Failure> {
    let workspace = open()?;

    Ok(ephemeral_engine::diagnostics(&workspace))
}

/// Containers no application accounts for, and their removal.
#[tauri::command]
fn leftovers(remove: bool) -> Result<Vec<String>, Failure> {
    let mut workspace = open()?;
    let runtime = ephemeral_engine::sandbox::usable_runtime(&workspace)
        .map_err(|error| format!("{error:#}"))?;

    let found = ephemeral_engine::orphans(&workspace, &runtime)
        .map_err(|error| format!("{error:#}"))?;
    let described: Vec<String> = found
        .iter()
        .map(|orphan| format!("{} — {}", orphan.container, orphan.reason))
        .collect();

    if remove {
        ephemeral_engine::remove_orphans(&mut workspace, &runtime, &found)
            .map_err(|error| format!("{error:#}"))?;
    }

    Ok(described)
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
            create,
            application,
            activity,
            decide,
            rollback,
            start,
            halt,
            refresh,
            logs,
            move_app,
            purge,
            authority,
            decide_authority,
            generate,
            generation,
            sweep,
            acknowledge,
            providers,
            diagnostics,
            leftovers,
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

    /// The frontend reaches Rust through `window.__TAURI__`, which Tauri v2
    /// only injects when this is set.
    ///
    /// Without it the window opens, renders its header, and then says "This
    /// window is not running inside Ephemeral" — while running inside
    /// Ephemeral. It shipped that way, and neither the Rust tests nor the
    /// headless rendering tests could see it: the commands were correct, the
    /// rendering was correct, and the two were never connected. Filming the
    /// real window under a virtual display is what found it, on the first
    /// frame.
    ///
    /// The alternative is importing `@tauri-apps/api`, which means a bundler,
    /// which means a build step and a supply chain for a window that shows a
    /// list. This flag is the price of not having one, so it is asserted rather
    /// than assumed.
    #[test]
    fn the_frontend_can_reach_rust() {
        let configuration: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json"))
                .expect("tauri.conf.json is valid JSON");

        assert_eq!(
            configuration["app"]["withGlobalTauri"],
            serde_json::Value::Bool(true),
            "the frontend calls window.__TAURI__ and has no bundler to import from"
        );
    }
}
