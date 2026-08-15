//! What a permission is *narrow to*.
//!
//! "Read the filesystem" is not a permission Ephemeral will ever ask for. "Read
//! everything under `~/Downloads/apartments`" is. The types here are what make
//! that difference expressible, and — more importantly — enforceable.
//!
//! ## Why this file is security-critical
//!
//! Prefix matching on paths is a classic source of escapes. `/home/user/docs`
//! must not permit `/home/user/docs-private`, `/home/user/docs/../.ssh`, or
//! `/home/user`. Getting any of those wrong turns a narrow grant into a broad
//! one, silently.
//!
//! So paths are normalised into segments at construction, `..` is rejected
//! outright rather than resolved, and containment is decided segment by segment
//! — never by string prefix.
//!
//! ## Why normalisation is lexical
//!
//! This crate performs no host I/O, so it cannot canonicalise a path against a
//! real filesystem, which means it cannot see through symbolic links. That is a
//! deliberate division of labour, not an oversight: the core decides *what was
//! granted*, and the runtime — which does touch the host — is responsible for
//! resolving links and refusing to mount anything that escapes the granted
//! scope. Both checks are required, and neither is sufficient alone.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Why a scope was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ScopeError {
    /// The scope was empty.
    #[error("scope is empty")]
    Empty,

    /// A path scope was not anchored to a root.
    ///
    /// Relative paths are refused because their meaning depends on a working
    /// directory, and a permission whose meaning depends on ambient state is not
    /// a permission.
    #[error(
        "path {path:?} is not anchored: it must start with '/', '~/' or a drive \
         letter such as 'C:/'"
    )]
    NotAnchored {
        /// The rejected path.
        path: String,
    },

    /// The path contained a `..` segment.
    ///
    /// Refused rather than resolved. Resolving it here would produce a scope
    /// whose written form differs from what the user approved.
    #[error("path {path:?} contains a '..' segment; scopes must be written out in full")]
    ParentTraversal {
        /// The rejected path.
        path: String,
    },

    /// The path contained a NUL byte, which no platform accepts and which can
    /// truncate a path when it reaches a C API.
    #[error("path {path:?} contains a NUL byte")]
    InteriorNul {
        /// The rejected path.
        path: String,
    },

    /// A host scope was not a plausible hostname.
    #[error("host {host:?} is not a valid hostname or wildcard pattern")]
    InvalidHost {
        /// The rejected host.
        host: String,
    },
}

/// Where a path scope is anchored.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Root {
    /// The filesystem root, `/`.
    Absolute,
    /// The user's home directory, written `~`. Resolved by the platform adapter,
    /// never by this crate.
    Home,
    /// A Windows drive, written `C:`. Stored uppercase.
    Drive(char),
}

impl Root {
    fn as_prefix(&self) -> String {
        match self {
            Self::Absolute => "/".to_owned(),
            Self::Home => "~/".to_owned(),
            Self::Drive(letter) => format!("{letter}:/"),
        }
    }
}

/// A region of the filesystem a permission applies to.
///
/// Written as an anchored path, optionally with a `/**` suffix meaning "and
/// everything beneath it":
///
/// | Written | Means |
/// |---------|-------|
/// | `~/Downloads/report.csv` | exactly that one path |
/// | `~/Downloads/apartments/**` | that directory and everything inside it |
/// | `/etc/hosts` | exactly that one path |
///
/// A scope without `/**` matches exactly one path. This matters: granting
/// `~/Downloads` alone does not grant anything *in* `~/Downloads`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathScope {
    root: Root,
    segments: Vec<String>,
    recursive: bool,
}

impl PathScope {
    /// Parses a written scope.
    ///
    /// Accepts forward or backward slashes, collapses repeated separators, drops
    /// `.` segments, and recognises a trailing `/**` as "and everything
    /// beneath".
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError`] if the path is empty, not anchored to a root,
    /// contains a `..` segment, or contains a NUL byte.
    pub fn parse(path: impl AsRef<str>) -> Result<Self, ScopeError> {
        let raw = path.as_ref().trim();

        if raw.is_empty() {
            return Err(ScopeError::Empty);
        }
        if raw.contains('\0') {
            return Err(ScopeError::InteriorNul {
                path: raw.to_owned(),
            });
        }

        let normalised = raw.replace('\\', "/");

        let (root, rest) = if let Some(rest) = normalised.strip_prefix("~/") {
            (Root::Home, rest)
        } else if normalised == "~" {
            (Root::Home, "")
        } else if let Some(rest) = normalised.strip_prefix('/') {
            (Root::Absolute, rest)
        } else if let Some(letter) = drive_letter(&normalised) {
            (Root::Drive(letter), &normalised[2..])
        } else {
            return Err(ScopeError::NotAnchored {
                path: raw.to_owned(),
            });
        };

        let mut segments = Vec::new();
        let mut recursive = false;

        for segment in rest.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    return Err(ScopeError::ParentTraversal {
                        path: raw.to_owned(),
                    });
                }
                "**" => recursive = true,
                other => {
                    if recursive {
                        // `**` is a suffix, not an infix: `/a/**/b` would be a
                        // pattern rather than a region, and regions are what a
                        // user can reason about.
                        return Err(ScopeError::NotAnchored {
                            path: raw.to_owned(),
                        });
                    }
                    segments.push(other.to_owned());
                }
            }
        }

        Ok(Self {
            root,
            segments,
            recursive,
        })
    }

    /// Whether this scope covers everything beneath its path.
    #[must_use]
    pub fn is_recursive(&self) -> bool {
        self.recursive
    }

    /// Whether this scope covers an entire root — `/**`, `~/**` or `C:/**`.
    ///
    /// Scopes like this are what SECURITY.md promises never to mount into a
    /// generated container, so they are flagged rather than merely allowed.
    #[must_use]
    pub fn is_whole_root(&self) -> bool {
        self.recursive && self.segments.is_empty()
    }

    /// Whether `other` lies entirely within this scope.
    ///
    /// Containment is decided segment by segment. A non-recursive scope contains
    /// only itself; a recursive scope contains itself and its descendants.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        if self.root != other.root {
            return false;
        }

        if self.recursive {
            // Every segment of this scope must be a leading segment of the
            // other. Comparing segments, not characters, is what stops
            // `/home/docs` from matching `/home/docs-private`.
            other.segments.len() >= self.segments.len()
                && self
                    .segments
                    .iter()
                    .zip(&other.segments)
                    .all(|(ours, theirs)| ours == theirs)
        } else {
            // A single path covers only itself, and only non-recursively: a
            // request for a whole subtree is not satisfied by a grant on one
            // path.
            !other.recursive && self.segments == other.segments
        }
    }

    /// The written form of this scope.
    #[must_use]
    pub fn as_written(&self) -> String {
        let mut out = self.root.as_prefix();
        out.push_str(&self.segments.join("/"));
        if self.recursive {
            if !self.segments.is_empty() {
                out.push('/');
            }
            out.push_str("**");
        }
        out
    }

    /// The path without the recursion marker, for display.
    ///
    /// `~/Downloads/apartments/**` displays as `~/Downloads/apartments`, which
    /// is what a permission prompt should show a person.
    #[must_use]
    pub fn display_path(&self) -> String {
        let mut out = self.root.as_prefix();
        out.push_str(&self.segments.join("/"));
        out
    }
}

impl fmt::Display for PathScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_written())
    }
}

impl std::str::FromStr for PathScope {
    type Err = ScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for PathScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_written())
    }
}

impl<'de> Deserialize<'de> for PathScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = String::deserialize(deserializer)?;
        Self::parse(&written).map_err(serde::de::Error::custom)
    }
}

/// A network destination a permission applies to.
///
/// Written as a hostname, optionally with a leading `*.` wildcard covering
/// subdomains, and optionally with a port:
///
/// | Written | Means |
/// |---------|-------|
/// | `api.example.com` | that host, any port |
/// | `api.example.com:443` | that host, that port only |
/// | `*.example.com` | any subdomain of `example.com`, but not `example.com` |
/// | `*` | anywhere — flagged as unrestricted |
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostScope {
    /// Lowercase host, or `*` for anywhere.
    host: String,
    /// Whether `host` is a `*.suffix` pattern.
    wildcard: bool,
    port: Option<u16>,
}

impl HostScope {
    /// Parses a written host scope.
    ///
    /// # Errors
    ///
    /// Returns [`ScopeError::InvalidHost`] if the host is not a plausible
    /// hostname or wildcard pattern, or the port is not a number.
    pub fn parse(host: impl AsRef<str>) -> Result<Self, ScopeError> {
        let raw = host.as_ref().trim();
        if raw.is_empty() {
            return Err(ScopeError::Empty);
        }

        let invalid = || ScopeError::InvalidHost {
            host: raw.to_owned(),
        };

        // Split a trailing `:port`, taking care not to mangle a bare IPv6
        // literal, which we do not accept in this form.
        let (name, port) = match raw.rsplit_once(':') {
            Some((name, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
                (name, Some(port.parse::<u16>().map_err(|_| invalid())?))
            }
            _ => (raw, None),
        };

        if name == "*" {
            return Ok(Self {
                host: "*".to_owned(),
                wildcard: false,
                port,
            });
        }

        let (name, wildcard) = match name.strip_prefix("*.") {
            Some(suffix) => (suffix, true),
            None => (name, false),
        };

        if name.is_empty() || name.len() > 253 || !name.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err(invalid());
        }
        if !name.chars().all(is_hostname_char)
            || name.starts_with('.')
            || name.ends_with('.')
            || name.contains("..")
        {
            return Err(invalid());
        }

        Ok(Self {
            host: name.to_ascii_lowercase(),
            wildcard,
            port,
        })
    }

    /// Whether this scope permits connections anywhere.
    #[must_use]
    pub fn is_anywhere(&self) -> bool {
        self.host == "*"
    }

    /// Whether `other` lies entirely within this scope.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        let port_ok = match (self.port, other.port) {
            // No port on the grant means any port.
            (None, _) => true,
            (Some(_), None) => false,
            (Some(ours), Some(theirs)) => ours == theirs,
        };
        if !port_ok {
            return false;
        }

        if self.is_anywhere() {
            return true;
        }
        if other.is_anywhere() {
            return false;
        }

        if self.wildcard {
            // `*.example.com` covers `a.example.com` and `a.b.example.com`, but
            // deliberately not `example.com` itself, and not `notexample.com`.
            if other.wildcard {
                other.host == self.host || other.host.ends_with(&format!(".{}", self.host))
            } else {
                other.host.ends_with(&format!(".{}", self.host))
            }
        } else {
            !other.wildcard && self.host == other.host
        }
    }

    /// The written form of this scope.
    #[must_use]
    pub fn as_written(&self) -> String {
        let name = if self.wildcard {
            format!("*.{}", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{name}:{port}"),
            None => name,
        }
    }
}

impl fmt::Display for HostScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_written())
    }
}

impl std::str::FromStr for HostScope {
    type Err = ScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for HostScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_written())
    }
}

impl<'de> Deserialize<'de> for HostScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = String::deserialize(deserializer)?;
        Self::parse(&written).map_err(serde::de::Error::custom)
    }
}

/// Recognises a `C:/` style prefix and returns the uppercase drive letter.
fn drive_letter(path: &str) -> Option<char> {
    let mut chars = path.chars();
    let letter = chars.next()?;
    if letter.is_ascii_alphabetic() && chars.next() == Some(':') {
        Some(letter.to_ascii_uppercase())
    } else {
        None
    }
}

fn is_hostname_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(path: &str) -> PathScope {
        PathScope::parse(path).unwrap_or_else(|e| panic!("{path} should parse: {e}"))
    }

    fn host(name: &str) -> HostScope {
        HostScope::parse(name).unwrap_or_else(|e| panic!("{name} should parse: {e}"))
    }

    // --- parsing ------------------------------------------------------------

    #[test]
    fn parses_the_three_anchors() {
        assert_eq!(scope("/etc/hosts").as_written(), "/etc/hosts");
        assert_eq!(scope("~/Downloads").as_written(), "~/Downloads");
        assert_eq!(scope("C:/Users/ana").as_written(), "C:/Users/ana");
        assert_eq!(
            scope("c:\\Users\\ana").as_written(),
            "C:/Users/ana",
            "Windows separators and drive case should normalise"
        );
    }

    #[test]
    fn collapses_redundant_separators_and_dots() {
        assert_eq!(scope("/a//b/./c").as_written(), "/a/b/c");
        assert_eq!(scope("~/a/").as_written(), "~/a");
    }

    #[test]
    fn recognises_the_recursion_marker() {
        let recursive = scope("~/Downloads/apartments/**");
        assert!(recursive.is_recursive());
        assert_eq!(recursive.as_written(), "~/Downloads/apartments/**");
        assert_eq!(
            recursive.display_path(),
            "~/Downloads/apartments",
            "prompts show the directory, not the glob"
        );
        assert!(!scope("~/Downloads/apartments").is_recursive());
    }

    /// `..` is refused rather than resolved: a scope must read the same as what
    /// the user approved.
    #[test]
    fn rejects_parent_traversal() {
        for hostile in [
            "/home/user/../../etc/shadow",
            "~/Downloads/../.ssh",
            "/a/b/..",
            "C:/Users/ana/../../Windows",
        ] {
            assert!(
                matches!(
                    PathScope::parse(hostile),
                    Err(ScopeError::ParentTraversal { .. })
                ),
                "{hostile} must be refused"
            );
        }
    }

    #[test]
    fn rejects_unanchored_and_malformed_paths() {
        assert!(matches!(PathScope::parse(""), Err(ScopeError::Empty)));
        for hostile in ["Downloads", "./relative", "../up", "a/b"] {
            assert!(
                matches!(
                    PathScope::parse(hostile),
                    Err(ScopeError::NotAnchored { .. })
                ),
                "{hostile} must be refused as unanchored"
            );
        }
        assert!(matches!(
            PathScope::parse("/a/\0/b"),
            Err(ScopeError::InteriorNul { .. })
        ));
        assert!(
            PathScope::parse("/a/**/b").is_err(),
            "'**' is a suffix, not an infix"
        );
    }

    // --- containment: the security-critical part ----------------------------

    /// The classic prefix bug. `/home/user/docs` must not cover
    /// `/home/user/docs-private`, because "docs" is not a prefix *segment* of
    /// "docs-private".
    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_contained() {
        let granted = scope("/home/user/docs/**");
        assert!(!granted.contains(&scope("/home/user/docs-private/secret")));
        assert!(!granted.contains(&scope("/home/user/docsX")));
        assert!(granted.contains(&scope("/home/user/docs/a.txt")));
    }

    #[test]
    fn a_recursive_scope_contains_itself_and_its_descendants() {
        let granted = scope("~/Downloads/apartments/**");
        assert!(granted.contains(&scope("~/Downloads/apartments")));
        assert!(granted.contains(&scope("~/Downloads/apartments/a.csv")));
        assert!(granted.contains(&scope("~/Downloads/apartments/2026/b.csv")));
        assert!(granted.contains(&scope("~/Downloads/apartments/**")));
    }

    #[test]
    fn a_recursive_scope_does_not_contain_its_parent_or_siblings() {
        let granted = scope("~/Downloads/apartments/**");
        assert!(!granted.contains(&scope("~/Downloads")));
        assert!(!granted.contains(&scope("~/Downloads/taxes/a.csv")));
        assert!(!granted.contains(&scope("~")));
    }

    /// Granting one path is not granting its contents. This is what makes
    /// `~/Downloads` a meaningfully different grant from `~/Downloads/**`.
    #[test]
    fn a_single_path_scope_covers_only_itself() {
        let granted = scope("~/Downloads/report.csv");
        assert!(granted.contains(&scope("~/Downloads/report.csv")));
        assert!(!granted.contains(&scope("~/Downloads/report.csv/inner")));
        assert!(!granted.contains(&scope("~/Downloads")));

        let directory = scope("~/Downloads");
        assert!(
            !directory.contains(&scope("~/Downloads/report.csv")),
            "granting a directory path must not grant its contents"
        );
        assert!(
            !directory.contains(&scope("~/Downloads/**")),
            "a single-path grant must not satisfy a request for a whole subtree"
        );
    }

    #[test]
    fn different_roots_never_contain_each_other() {
        assert!(!scope("/**").contains(&scope("~/a")));
        assert!(!scope("~/**").contains(&scope("/home/user/a")));
        assert!(!scope("C:/**").contains(&scope("D:/a")));
        assert!(!scope("~/**").contains(&scope("C:/Users/ana")));
    }

    #[test]
    fn whole_root_scopes_are_recognisable() {
        assert!(scope("/**").is_whole_root());
        assert!(scope("~/**").is_whole_root());
        assert!(scope("C:/**").is_whole_root());
        assert!(!scope("~/Downloads/**").is_whole_root());
        assert!(!scope("/").is_whole_root());
    }

    #[test]
    fn path_scopes_round_trip_through_json() {
        for written in [
            "/etc/hosts",
            "~/Downloads/apartments/**",
            "C:/Users/ana/**",
            "~/**",
        ] {
            let parsed = scope(written);
            let json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, format!("\"{written}\""));
            assert_eq!(serde_json::from_str::<PathScope>(&json).unwrap(), parsed);
        }
    }

    /// Deserialisation is not a way around the parser: a manifest arriving from
    /// disk gets the same refusals.
    #[test]
    fn deserialisation_rejects_hostile_paths() {
        assert!(serde_json::from_str::<PathScope>("\"~/../../etc\"").is_err());
        assert!(serde_json::from_str::<PathScope>("\"relative/path\"").is_err());
    }

    // --- host scopes --------------------------------------------------------

    #[test]
    fn parses_hosts_wildcards_and_ports() {
        assert_eq!(host("api.example.com").as_written(), "api.example.com");
        assert_eq!(host("API.Example.COM").as_written(), "api.example.com");
        assert_eq!(host("*.example.com").as_written(), "*.example.com");
        assert_eq!(
            host("api.example.com:443").as_written(),
            "api.example.com:443"
        );
        assert!(host("*").is_anywhere());
    }

    #[test]
    fn rejects_malformed_hosts() {
        for hostile in [
            "",
            "exa mple.com",
            "exa_mple.com",
            ".example.com",
            "a..b.com",
        ] {
            assert!(
                HostScope::parse(hostile).is_err(),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn a_wildcard_covers_subdomains_but_not_the_apex() {
        let granted = host("*.example.com");
        assert!(granted.contains(&host("api.example.com")));
        assert!(granted.contains(&host("a.b.example.com")));
        assert!(
            !granted.contains(&host("example.com")),
            "the apex is a different host and must be granted separately"
        );
        assert!(
            !granted.contains(&host("notexample.com")),
            "suffix matching must respect the dot boundary"
        );
        assert!(!granted.contains(&host("example.com.attacker.net")));
    }

    #[test]
    fn an_exact_host_covers_only_itself() {
        let granted = host("api.example.com");
        assert!(granted.contains(&host("api.example.com")));
        assert!(!granted.contains(&host("other.example.com")));
        assert!(!granted.contains(&host("*.example.com")));
        assert!(!granted.contains(&host("*")));
    }

    #[test]
    fn ports_narrow_a_host_scope() {
        let any_port = host("api.example.com");
        let https = host("api.example.com:443");

        assert!(
            any_port.contains(&https),
            "no port on the grant means any port"
        );
        assert!(
            !https.contains(&any_port),
            "a port-specific grant must not satisfy an any-port request"
        );
        assert!(!https.contains(&host("api.example.com:80")));
        assert!(https.contains(&https));
    }

    #[test]
    fn anywhere_covers_everything_and_nothing_covers_anywhere() {
        assert!(host("*").contains(&host("api.example.com")));
        assert!(host("*").contains(&host("*.example.com")));
        assert!(!host("*.example.com").contains(&host("*")));
        assert!(!host("api.example.com").contains(&host("*")));
    }

    #[test]
    fn host_scopes_round_trip_through_json() {
        for written in [
            "api.example.com",
            "*.example.com",
            "api.example.com:443",
            "*",
        ] {
            let parsed = host(written);
            let json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, format!("\"{written}\""));
            assert_eq!(serde_json::from_str::<HostScope>(&json).unwrap(), parsed);
        }
    }
}
