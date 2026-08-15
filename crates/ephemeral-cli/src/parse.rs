//! Turning what somebody typed into a permission.
//!
//! Permissions are the one thing a user of the CLI types that has security
//! consequences, so the syntax is short enough to type and explicit enough that
//! it cannot be mistaken for something narrower than it is.
//!
//! ```text
//! read:~/Downloads/apartments/**    read that folder and everything in it
//! read:~/Downloads/report.csv       read exactly that one file
//! write:~/reports/**                create and change files there
//! net:api.example.com               connect to that host
//! net:*.example.com:443             connect to its subdomains, on 443 only
//! net:*                             connect anywhere — flagged as such
//! port:8080                         accept connections on a port
//! env:API_KEY                       use a setting, without seeing it here
//! camera | microphone | location    device access
//! execute                           run other programs
//! ```
//!
//! Anything unrecognised is rejected with the list of what is accepted. A
//! permission grammar that silently ignores a typo would grant something other
//! than what the user wrote.

use anyhow::{Context as _, Result, bail};
use ephemeral_core::{
    identity::{AppId, Principal},
    permission::{AppPermission, HostScope, MetaPermission, PathScope, Permission},
};

/// The word that means Ephemeral itself rather than a generated application.
///
/// Reserved: generated ids always carry a random suffix, so this cannot collide
/// with one in practice.
pub(crate) const SELF_PRINCIPAL: &str = "ephemeral";

/// Parses the subject of a grant.
///
/// # Errors
///
/// If the value is neither the reserved word nor a valid application id.
pub(crate) fn principal(value: &str) -> Result<Principal> {
    if value == SELF_PRINCIPAL {
        return Ok(Principal::Ephemeral);
    }

    AppId::parse(value)
        .map(Principal::app)
        .with_context(|| format!("{value:?} is not an application id"))
}

/// Parses a permission for whichever space the principal belongs to.
///
/// The two permission systems are separate, so the same written form can mean
/// different things depending on who is being granted it — `camera` for
/// Ephemeral is the product's access to the device, `camera` for an app is that
/// one app's.
///
/// # Errors
///
/// If the text is not a permission this principal can hold.
pub(crate) fn permission(subject: &Principal, value: &str) -> Result<Permission> {
    if subject.is_ephemeral() {
        meta(value).map(Permission::Meta)
    } else {
        app(value).map(Permission::App)
    }
}

/// Parses an application permission.
///
/// # Errors
///
/// If the text is not a recognised application permission.
pub(crate) fn app(value: &str) -> Result<AppPermission> {
    let (head, tail) = split(value);

    Ok(match (head, tail) {
        ("read", Some(path)) => AppPermission::read(path_scope(path)?),
        ("write", Some(path)) => AppPermission::write(path_scope(path)?),
        ("net" | "network", Some(host)) => AppPermission::outbound(host_scope(host)?),
        ("port", Some(port)) => AppPermission::NetworkInbound {
            port: port
                .parse()
                .with_context(|| format!("{port:?} is not a port number"))?,
        },
        ("env", Some(name)) => AppPermission::ReadEnvironment {
            name: name.to_owned(),
        },
        ("execute", None) => AppPermission::ExecuteProcesses,
        ("camera", None) => AppPermission::Camera,
        ("microphone" | "mic", None) => AppPermission::Microphone,
        ("location", None) => AppPermission::Location,
        _ => bail!(
            "{value:?} is not an application permission.\n\
             \n\
             Try one of:\n  \
             read:<path>        e.g. read:~/Downloads/apartments/**\n  \
             write:<path>       e.g. write:~/reports/**\n  \
             net:<host>         e.g. net:api.example.com  or  net:*\n  \
             port:<number>      e.g. port:8080\n  \
             env:<NAME>         e.g. env:API_KEY\n  \
             execute, camera, microphone, location"
        ),
    })
}

/// Parses a meta-permission — something Ephemeral itself may do.
///
/// # Errors
///
/// If the text is not a recognised meta-permission.
pub(crate) fn meta(value: &str) -> Result<MetaPermission> {
    let (head, tail) = split(value);

    Ok(match (head, tail) {
        ("read", Some(path)) => MetaPermission::read(path_scope(path)?),
        ("write", Some(path)) => MetaPermission::write(path_scope(path)?),
        ("execute", None) => MetaPermission::ExecuteProcesses,
        ("install-deps", None) => MetaPermission::InstallDependencies,
        ("network", None) => MetaPermission::NetworkAccess,
        ("docker", None) => MetaPermission::UseDocker,
        ("docker-install", None) => MetaPermission::InstallDocker,
        ("pull-images", None) => MetaPermission::PullImages,
        ("env", None) => MetaPermission::ReadEnvironment,
        ("keychain", None) => MetaPermission::AccessKeychain,
        ("credentials", None) => MetaPermission::AccessCredentials,
        ("shortcuts", None) => MetaPermission::CreateShortcuts,
        ("notifications", None) => MetaPermission::SendNotifications,
        ("camera", None) => MetaPermission::Camera,
        ("microphone" | "mic", None) => MetaPermission::Microphone,
        ("location", None) => MetaPermission::Location,
        ("contacts", None) => MetaPermission::Contacts,
        ("calendar", None) => MetaPermission::Calendar,
        ("browser-data", None) => MetaPermission::BrowserData,
        ("devices", None) => MetaPermission::ExternalDevices,
        ("self-update", None) => MetaPermission::SelfUpdate,
        _ => bail!(
            "{value:?} is not something Ephemeral itself can be granted.\n\
             \n\
             Try one of:\n  \
             read:<path>, write:<path>\n  \
             docker, docker-install, pull-images, network, execute, install-deps\n  \
             env, keychain, credentials, shortcuts, notifications\n  \
             camera, microphone, location, contacts, calendar, browser-data, devices\n  \
             self-update"
        ),
    })
}

/// Splits `head:tail`, taking care not to split a Windows drive letter or a
/// `host:port` off the wrong colon.
fn split(value: &str) -> (&str, Option<&str>) {
    match value.split_once(':') {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    }
}

fn path_scope(value: &str) -> Result<PathScope> {
    PathScope::parse(value).with_context(|| format!("{value:?} is not a usable path"))
}

fn host_scope(value: &str) -> Result<HostScope> {
    HostScope::parse(value).with_context(|| format!("{value:?} is not a usable host"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap()
    }

    #[test]
    fn the_reserved_word_means_ephemeral_itself() {
        assert_eq!(principal("ephemeral").unwrap(), Principal::Ephemeral);
        assert_eq!(
            principal("csv-comparator").unwrap(),
            Principal::app(AppId::parse("csv-comparator").unwrap())
        );
    }

    /// A principal that could escape its storage directory must be refused here
    /// too, not only deeper in.
    #[test]
    fn a_hostile_principal_is_refused() {
        assert!(principal("../../etc").is_err());
        assert!(principal("Not Valid").is_err());
    }

    #[test]
    fn application_permissions_parse() {
        assert_eq!(
            app("read:~/Downloads/apartments/**").unwrap(),
            AppPermission::read(scope("~/Downloads/apartments/**"))
        );
        assert_eq!(
            app("write:~/reports/**").unwrap(),
            AppPermission::write(scope("~/reports/**"))
        );
        assert_eq!(
            app("net:api.example.com").unwrap(),
            AppPermission::outbound(HostScope::parse("api.example.com").unwrap())
        );
        assert_eq!(
            app("port:8080").unwrap(),
            AppPermission::NetworkInbound { port: 8080 }
        );
        assert_eq!(
            app("env:API_KEY").unwrap(),
            AppPermission::ReadEnvironment {
                name: "API_KEY".to_owned()
            }
        );
        assert_eq!(app("execute").unwrap(), AppPermission::ExecuteProcesses);
        assert_eq!(app("camera").unwrap(), AppPermission::Camera);
        assert_eq!(app("mic").unwrap(), AppPermission::Microphone);
    }

    /// A host scope keeps its own port, rather than the first colon winning.
    #[test]
    fn a_host_with_a_port_survives_the_split() {
        assert_eq!(
            app("net:api.example.com:443").unwrap(),
            AppPermission::outbound(HostScope::parse("api.example.com:443").unwrap())
        );
    }

    #[test]
    fn meta_permissions_parse() {
        assert_eq!(meta("docker").unwrap(), MetaPermission::UseDocker);
        assert_eq!(
            meta("docker-install").unwrap(),
            MetaPermission::InstallDocker
        );
        assert_eq!(meta("self-update").unwrap(), MetaPermission::SelfUpdate);
        assert_eq!(
            meta("read:~/**").unwrap(),
            MetaPermission::read(scope("~/**"))
        );
    }

    /// The same word means different things in the two permission spaces, and
    /// the principal decides which — this is what stops an app permission being
    /// granted to Ephemeral or the reverse.
    #[test]
    fn the_principal_decides_which_space_is_parsed() {
        let for_app = permission(
            &Principal::app(AppId::parse("csv-comparator").unwrap()),
            "camera",
        )
        .unwrap();
        let for_ephemeral = permission(&Principal::Ephemeral, "camera").unwrap();

        assert!(matches!(for_app, Permission::App(_)));
        assert!(matches!(for_ephemeral, Permission::Meta(_)));
        assert!(!for_app.satisfies(&for_ephemeral));
    }

    /// A permission Ephemeral can hold but an app cannot must not quietly parse
    /// as something else.
    #[test]
    fn permissions_do_not_cross_between_the_two_spaces() {
        assert!(app("docker").is_err());
        assert!(app("self-update").is_err());
        assert!(meta("port:8080").is_err());
    }

    /// A typo must be an error. Silently ignoring one would grant something
    /// other than what was typed.
    #[test]
    fn a_typo_is_refused_with_help() {
        let error = app("raed:~/Downloads").unwrap_err().to_string();
        assert!(error.contains("not an application permission"));
        assert!(
            error.contains("read:<path>"),
            "the error should say what works"
        );

        assert!(app("").is_err());
        assert!(app("read").is_err(), "a scoped permission needs its scope");
        assert!(app("read:relative/path").is_err());
        assert!(app("read:~/../../etc/shadow").is_err());
        assert!(app("port:not-a-number").is_err());
    }
}
