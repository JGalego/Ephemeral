//! Carrying a confined application's request, on a machine with a terminal.
//!
//! A WebAssembly application has no socket and never will
//! ([ADR-0021](../../../docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md)).
//! What it can do, once a person has allowed it, is *describe* a request; this
//! is the half that makes one, on a desktop, by spawning `curl` — the same
//! decision, for the same reasons, as the transport that carries a request to a
//! model provider (ADR-0016). No HTTP client is linked into Ephemeral; TLS,
//! certificate policy and proxy settings stay with the tool the operating
//! system already ships and the administrator already configures.
//!
//! **This decides nothing.** Whether the destination is one the application may
//! reach was settled before it got here, in
//! [`ephemeral_runtime::wasm`], against the grant the person made. A copy of
//! that decision here would be a second permission model, and the copy that
//! drifts is the one nobody is looking at.

use std::io::Write as _;
use std::process::{Command, Stdio};

use ephemeral_runtime::wasm::{Answered, Method, Outbound, Reach};

/// How long one request may take before it is abandoned.
///
/// Short, on purpose, and much shorter than the model transport's: this is a
/// request an application made while somebody watches it run, and the whole run
/// is bounded. A generated application that hangs on a slow endpoint should
/// fail rather than hold the terminal.
const SECONDS: u32 = 30;

/// The most bytes of reply to accept.
///
/// `curl` is told, so an endpoint that answers with a stream is cut off by the
/// tool rather than filling this process's memory first.
const MOST: usize = ephemeral_runtime::wasm::MOST_ONE_BODY;

/// Makes a confined application's request by spawning `curl`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Curl;

impl Reach for Curl {
    fn fetch(&self, request: &Outbound) -> Result<Answered, String> {
        let mut child = Command::new("curl")
            .args(arguments(request))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => {
                    "curl is not installed, so nothing here can carry a request".to_owned()
                }
                _ => format!("curl could not be started: {error}"),
            })?;

        // The body goes in on stdin rather than in an argument. Arguments are
        // readable by anyone who can list processes, and a message somebody
        // typed is theirs.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(request.body.as_bytes());
        }

        let finished = child
            .wait_with_output()
            .map_err(|error| format!("curl could not be waited for: {error}"))?;

        if !finished.status.success() {
            let said = String::from_utf8_lossy(&finished.stderr);
            let said = said.trim();
            return Err(if said.is_empty() {
                format!("{} could not be reached", request.url)
            } else {
                format!("{} could not be reached: {said}", request.url)
            });
        }

        Ok(parsed(&String::from_utf8_lossy(&finished.stdout)))
    }
}

/// What `curl` is asked to do.
///
/// A pure function, so what is asked of the network is a test rather than a
/// claim, and so a reader can see there is nothing here but a method, a URL and
/// a content type.
#[must_use]
pub fn arguments(request: &Outbound) -> Vec<String> {
    let mut arguments = vec![
        // Named rather than inferred. curl decides the method from whether it
        // was given data, so a GET that acquired a body would silently become a
        // POST.
        "--request".to_owned(),
        request.method.as_str().to_owned(),
        "--silent".to_owned(),
        "--show-error".to_owned(),
        // The status, so an application can tell 404 from 200 rather than
        // guessing from a body.
        "--write-out".to_owned(),
        "\n%{http_code}".to_owned(),
        "--max-time".to_owned(),
        SECONDS.to_string(),
        "--max-filesize".to_owned(),
        MOST.to_string(),
        // A redirect is a different destination, and the destination is the
        // thing a person approved. Following one would reach somewhere nobody
        // was asked about.
        "--no-location".to_owned(),
        // Refused rather than negotiated: an application's traffic is not
        // allowed to fall back to something unencrypted.
        "--proto".to_owned(),
        "=https,http".to_owned(),
    ];

    if request.method == Method::Post {
        arguments.extend([
            "--header".to_owned(),
            "content-type: application/json".to_owned(),
            "--data-binary".to_owned(),
            "@-".to_owned(),
        ]);
    }

    arguments.push(request.url.clone());
    arguments
}

/// Splits curl's output into the body and the status it appended.
fn parsed(written: &str) -> Answered {
    match written.rsplit_once('\n') {
        Some((body, status)) => Answered {
            status: status.trim().parse().unwrap_or(0),
            body: body.to_owned(),
        },
        // No newline at all means nothing but the status came back, which is
        // what an empty reply looks like.
        None => Answered {
            status: written.trim().parse().unwrap_or(0),
            body: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asking(method: Method, url: &str) -> Outbound {
        Outbound {
            method,
            url: url.to_owned(),
            body: "{\"said\":\"hello\"}".to_owned(),
        }
    }

    /// The body is never an argument. Anybody who can list processes can read
    /// an argument vector, and what somebody typed into an application is
    /// theirs.
    #[test]
    fn nothing_a_person_typed_becomes_an_argument() {
        let arguments = arguments(&asking(Method::Post, "https://api.example.com/say"));

        assert!(
            !arguments.iter().any(|argument| argument.contains("hello")),
            "{arguments:?}"
        );
        assert!(
            arguments.iter().any(|argument| argument == "@-"),
            "it is read from stdin instead: {arguments:?}"
        );
    }

    /// A GET carries no body, and curl is told the method rather than left to
    /// infer it from whether there was one.
    #[test]
    fn the_method_is_named_rather_than_inferred() {
        let reading = arguments(&asking(Method::Get, "https://api.example.com/read"));

        assert_eq!(reading[0], "--request");
        assert_eq!(reading[1], "GET");
        assert!(
            !reading.iter().any(|argument| argument == "--data-binary"),
            "a GET sends nothing: {reading:?}"
        );
    }

    /// A redirect is a different destination, and only the first one was
    /// approved.
    #[test]
    fn a_redirect_is_not_followed_to_somewhere_nobody_approved() {
        let arguments = arguments(&asking(Method::Get, "https://api.example.com/read"));

        assert!(arguments.iter().any(|argument| argument == "--no-location"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "--location" || argument == "-L"),
            "{arguments:?}"
        );
    }

    #[test]
    fn the_status_comes_back_beside_the_body() {
        let answered = parsed("{\"ok\":true}\n200");

        assert_eq!(answered.status, 200);
        assert_eq!(answered.body, "{\"ok\":true}");
    }

    /// A body with newlines in it keeps them. Splitting on the first newline
    /// rather than the last would truncate every multi-line reply.
    #[test]
    fn a_reply_with_newlines_in_it_survives() {
        let answered = parsed("first\nsecond\nthird\n404");

        assert_eq!(answered.status, 404);
        assert_eq!(answered.body, "first\nsecond\nthird");
    }
}
