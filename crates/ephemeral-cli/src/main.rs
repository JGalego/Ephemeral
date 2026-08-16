//! The Ephemeral command line.
//!
//! The CLI is not a wrapper around the desktop application: both are clients of
//! the same domain model in `ephemeral-core`, so anything one can express the
//! other can too, and a permission decision means the same thing in both
//! ([`ARCHITECTURE.md` §5](https://github.com/JGalego/Ephemeral/blob/main/ARCHITECTURE.md)).
//!
//! Phase 1 gives you everything that does not need a model provider: creating
//! an application record, inspecting it, moving it through its lifecycle,
//! granting and revoking permissions, running it in a container under exactly
//! those permissions, reading the audit trail, and diagnosing the environment.
//! Generating an application from a description arrives in Phase 2, and the
//! commands that need it say so plainly rather than pretending.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod commands;
mod doctor;
mod generate;
mod output;
mod parse;
mod review;
mod runtime;
mod share;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};

/// Where Ephemeral keeps its state, unless told otherwise.
const HOME_ENV: &str = "EPHEMERAL_HOME";

#[derive(Parser)]
#[command(
    name = "ephemeral",
    version,
    about = "Software that exists only while it's useful.",
    long_about = "Ephemeral builds small applications from a description, runs them in a \
                  sandbox, and throws them away when you're done.\n\n\
                  This is Phase 1: the application model, the lifecycle, the permission \
                  system and the container sandbox are here. Generating an application from \
                  a description is not yet — see docs/roadmap.md.",
    propagate_version = true
)]
struct Cli {
    /// Where Ephemeral keeps its applications, permissions and audit log.
    #[arg(long, global = true, env = HOME_ENV, value_name = "PATH")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ask for an application.
    #[command(
        long_about = "Records what you want. Generation arrives in Phase 2; until then \
                            this creates the application's identity, intent and retention \
                            policy so the rest of its life can be exercised."
    )]
    Create {
        /// What you want the application to do, in your own words.
        intent: String,

        /// A name for it. Derived from the intent if you don't give one.
        #[arg(long)]
        name: Option<String>,

        /// How long to keep it: one-shot, ephemeral, temporary, reusable, persistent.
        #[arg(long, default_value = "temporary")]
        retention: String,
    },

    /// List your applications.
    List {
        /// Include archived and deleted ones.
        #[arg(long, short)]
        all: bool,
    },

    /// Show everything about one application.
    Inspect {
        /// Which application.
        app: String,
    },

    /// Show what an application is allowed to do.
    Permissions {
        /// Which application, or `ephemeral` for the product itself.
        app: String,
    },

    /// Allow something.
    #[command(
        long_about = "Grants a permission. Only you can do this — no autonomous part of \
                            Ephemeral can grant itself or anything else access."
    )]
    Grant {
        /// Which application, or `ephemeral` for the product itself.
        app: String,
        /// What to allow, e.g. `read:~/Downloads/**`.
        permission: String,
        /// Why you are allowing it. Recorded in the audit log.
        #[arg(long)]
        why: Option<String>,
    },

    /// Take a permission back.
    Revoke {
        /// Which application, or `ephemeral` for the product itself.
        app: String,
        /// What to revoke.
        permission: String,
    },

    /// Put an application away, keeping its data.
    Archive {
        /// Which application.
        app: String,
    },

    /// Bring an archived application back.
    Restore {
        /// Which application.
        app: String,
    },

    /// Delete an application: destroy its runtime access and revoke everything.
    #[command(
        long_about = "Deleting withdraws every permission immediately and stops the \
                            application doing anything. Its record and data are kept so you \
                            can restore it; `purge` is what destroys them."
    )]
    Delete {
        /// Which application.
        app: String,
    },

    /// Destroy an application and all its data, irreversibly.
    Purge {
        /// Which application.
        app: String,
        /// Confirm that you mean it.
        #[arg(long)]
        yes: bool,
    },

    /// Show an application's history, and what it is printing now.
    Logs {
        /// Which application.
        app: String,
        /// How many lines of the application's own output to show.
        #[arg(long, default_value_t = 50)]
        lines: u32,
    },

    /// Show the security record.
    Audit {
        /// Only entries for this application.
        #[arg(long)]
        app: Option<String>,
        /// How many of the most recent entries to show.
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },

    /// Build an application from the intent already recorded for it.
    Generate {
        /// Which application.
        app: String,
        /// Which model provider to use: `mock` or `anthropic`.
        #[arg(long, default_value = "mock")]
        provider: String,
    },

    /// Return an application to a version it used to be.
    #[command(
        long_about = "Puts an earlier version's source back and records the change.\n\n\
                      The built image is cleared, because a version is its source: \
                      running the newer build under this version's name would report \
                      one thing and run another. Generate again to rebuild.\n\n\
                      If the older version asks for a capability the newer one had \
                      stopped needing, the grants for it are withdrawn — an approval \
                      given for different code is not an approval for this one."
    )]
    Rollback {
        /// Which application.
        app: String,
        /// Which version, by digest or an unambiguous prefix of one.
        ///
        /// Named `digest` rather than `version` because clap generates a
        /// `--version` flag and the two collide — which is a panic on startup,
        /// not a compile error, and so was invisible until the binary was
        /// actually run.
        digest: String,
    },

    /// Decide what an application may do, one question at a time.
    Review {
        /// Which application.
        app: String,
    },

    /// Write an application out for somebody else to build.
    Publish {
        /// Which application.
        app: String,
        /// Where to write it.
        into: PathBuf,
    },

    /// Look at an application somebody published, and accept it if you want it.
    Install {
        /// The directory it was published to.
        from: PathBuf,
        /// Accept it. Without this, it only shows you what it is.
        #[arg(long)]
        accept: bool,
    },

    /// Show the lifecycle state machine, or where one application sits in it.
    States {
        /// Which application. Omit to see the machine itself.
        app: Option<String>,
    },

    /// Check this machine for problems.
    Doctor,

    /// Start an application.
    Run {
        /// Which application.
        app: String,
        /// Arguments for the application itself, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },

    /// Stop a running application.
    Stop {
        /// Which application.
        app: String,
    },

    /// Suspend a running application.
    Pause {
        /// Which application.
        app: String,
    },

    /// Pick a suspended application back up.
    Resume {
        /// Which application.
        app: String,
    },

    /// Check what an application is actually doing, and correct its record.
    Status {
        /// Which application.
        app: String,
    },

    /// Watch running applications: notice crashes and apply time limits.
    Watch {
        /// How often to look, in seconds.
        #[arg(long, default_value_t = 15)]
        interval: u64,
        /// Look once and exit, instead of watching.
        #[arg(long)]
        once: bool,
    },

    /// Remove containers left behind by a crash or a deleted application.
    Cleanup {
        /// Actually remove them. Without this, it only says what it would do.
        #[arg(long)]
        yes: bool,
    },
}

/// Where each platform expects an application to keep its data.
///
/// Written out rather than taken from a crate: the obvious dependency for this
/// pulls in a weak-copyleft transitive dependency, and the supply-chain policy
/// in `deny.toml` allows permissive licences only. Thirty lines of documented
/// path logic is a better trade than an exception to that policy, and it is one
/// fewer thing in the dependency tree of a product that runs untrusted code.
///
/// Taking the environment as arguments rather than reading it makes this
/// testable on every platform at once, instead of only on whichever one CI
/// happens to be running.
fn data_dir_for(
    os: &str,
    home: Option<&Path>,
    xdg_data_home: Option<&Path>,
    appdata: Option<&Path>,
) -> Option<PathBuf> {
    match os {
        "windows" => appdata.map(|base| base.join("Ephemeral")),
        "macos" => home.map(|base| base.join("Library/Application Support/Ephemeral")),
        _ => {
            // The XDG spec says a relative XDG_DATA_HOME is invalid and must be
            // ignored, rather than resolved against the working directory.
            let base = xdg_data_home
                .filter(|path| is_posix_absolute(path))
                .map(Path::to_path_buf)
                .or_else(|| home.map(|base| base.join(".local/share")))?;
            Some(base.join("ephemeral"))
        }
    }
}

/// Whether a path is absolute by POSIX rules.
///
/// [`Path::is_absolute`] applies the rules of whatever platform the code was
/// compiled for, so `/data/xdg` is *not* absolute on Windows — it has no drive.
/// XDG is a POSIX specification and its branch only ever applies on Unix, so it
/// is evaluated by POSIX rules wherever this happens to be compiled. That also
/// restores the property [`data_dir_for`] claims: every platform's behaviour is
/// testable from any one of them.
fn is_posix_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|text| text.starts_with('/'))
}

/// Resolves where Ephemeral keeps its state.
///
/// `--home` or `EPHEMERAL_HOME` wins; otherwise the platform's own data
/// directory, so Ephemeral puts its files where each operating system expects
/// rather than scattering dotfiles.
fn resolve_home(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);

    data_dir_for(
        std::env::consts::OS,
        home.as_deref(),
        xdg.as_deref(),
        appdata.as_deref(),
    )
    .context(
        "could not work out where to keep Ephemeral's files on this system; \
         set EPHEMERAL_HOME or pass --home",
    )
}

fn run(cli: Cli) -> Result<()> {
    let home = resolve_home(cli.home)?;

    match cli.command {
        Command::Create {
            intent,
            name,
            retention,
        } => commands::create(&home, &intent, name.as_deref(), &retention),
        Command::List { all } => commands::list(&home, all),
        Command::Inspect { app } => commands::inspect(&home, &app),
        Command::Permissions { app } => commands::permissions(&home, &app),
        Command::Grant {
            app,
            permission,
            why,
        } => commands::grant(&home, &app, &permission, why.as_deref()),
        Command::Revoke { app, permission } => commands::revoke(&home, &app, &permission),
        Command::Archive { app } => commands::archive(&home, &app),
        Command::Restore { app } => commands::restore(&home, &app),
        Command::Delete { app } => commands::delete(&home, &app),
        Command::Purge { app, yes } => commands::purge(&home, &app, yes),
        Command::Logs { app, lines } => commands::logs(&home, &app, lines),
        Command::Audit { app, limit } => commands::audit(&home, app.as_deref(), limit),
        Command::Generate { app, provider } => generate::run(&home, &app, &provider),
        Command::Rollback { app, digest } => commands::rollback(&home, &app, &digest),
        Command::Review { app } => review::run(&home, &app),
        Command::Publish { app, into } => share::publish(&home, &app, &into),
        Command::Install { from, accept } => share::install(&home, &from, accept),
        Command::States { app } => commands::states(&home, app.as_deref()),
        Command::Doctor => {
            doctor::run(&home);
            Ok(())
        }
        Command::Run { app, arguments } => runtime::run(&home, &app, &arguments),
        Command::Stop { app } => runtime::stop(&home, &app),
        Command::Pause { app } => runtime::pause(&home, &app),
        Command::Resume { app } => runtime::resume(&home, &app),
        Command::Status { app } => runtime::status(&home, &app),
        Command::Watch { interval, once } => runtime::watch(&home, interval, once),
        Command::Cleanup { yes } => runtime::cleanup(&home, yes),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{} {error}", output::bad("error:"));
            // Causes matter here: "could not save the ledger" is not actionable
            // without "permission denied on /var/lib/ephemeral".
            for cause in error.chain().skip(1) {
                eprintln!("  {} {cause}", output::dim("caused by:"));
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    /// clap's own consistency checks: duplicate names, bad argument
    /// combinations, missing help. Cheap, and catches a whole class of mistakes
    /// that would otherwise only show up when somebody runs the command.
    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn an_explicit_home_wins_over_the_platform_default() {
        let explicit = PathBuf::from("/tmp/somewhere");
        assert_eq!(resolve_home(Some(explicit.clone())).unwrap(), explicit);
    }

    #[test]
    fn each_platform_gets_the_directory_it_expects() {
        let home = Path::new("/home/ana");
        let appdata = Path::new("C:/Users/ana/AppData/Roaming");

        assert_eq!(
            data_dir_for("linux", Some(home), None, None).unwrap(),
            PathBuf::from("/home/ana/.local/share/ephemeral")
        );
        assert_eq!(
            data_dir_for("macos", Some(Path::new("/Users/ana")), None, None).unwrap(),
            PathBuf::from("/Users/ana/Library/Application Support/Ephemeral")
        );
        assert_eq!(
            data_dir_for("windows", None, None, Some(appdata)).unwrap(),
            PathBuf::from("C:/Users/ana/AppData/Roaming/Ephemeral")
        );
    }

    #[test]
    fn xdg_data_home_is_honoured_when_it_is_set() {
        assert_eq!(
            data_dir_for(
                "linux",
                Some(Path::new("/home/ana")),
                Some(Path::new("/data/xdg")),
                None
            )
            .unwrap(),
            PathBuf::from("/data/xdg/ephemeral")
        );
    }

    /// The XDG specification says a relative value is invalid and must be
    /// ignored rather than resolved against the working directory — which would
    /// put a user's applications wherever they happened to be standing.
    ///
    /// "Relative" here means by POSIX rules, not by the rules of whichever
    /// platform is running the test: `Path::is_absolute` called this Unix path
    /// relative on Windows, which sent CI red and the user's data somewhere
    /// they did not ask for.
    #[test]
    fn a_relative_xdg_data_home_is_ignored() {
        assert_eq!(
            data_dir_for(
                "linux",
                Some(Path::new("/home/ana")),
                Some(Path::new("relative/path")),
                None
            )
            .unwrap(),
            PathBuf::from("/home/ana/.local/share/ephemeral")
        );
    }

    #[test]
    fn nowhere_to_put_anything_is_an_error_rather_than_a_guess() {
        assert_eq!(data_dir_for("linux", None, None, None), None);
        assert_eq!(
            data_dir_for("windows", Some(Path::new("/home/ana")), None, None),
            None
        );
    }
}
