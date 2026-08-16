//! The one part of this crate CI cannot exercise.
//!
//! Deliberately small, and deliberately boring. Everything that decides
//! anything is in [`crate::wire`], which is pure and tested; what is left here
//! carries a string to the API and reads what comes back ([ADR-0016]).
//!
//! **No secret ever appears in an argument vector.** The credential travels in
//! a `--config` document on stdin, alongside the request body, so the command
//! line is safe to record verbatim in the audit log and safe to show a user who
//! asks what was sent. This is the same property the container arguments have,
//! for the same reason.
//!
//! ## Why this is a trait
//!
//! Spawning `curl` was the whole transport, and that quietly decided where
//! Ephemeral could generate. iOS does not allow a process to spawn another
//! process — there is no `fork`, no `exec`, and no `curl` binary to reach — so
//! a provider that can only talk to the network through a subprocess cannot
//! generate on a phone at all.
//!
//! That was never an intended constraint. Generating is an HTTPS request:
//! what iOS forbids is *executing* newly written code, which is a different
//! thing and belongs behind the runtime seam ([ADR-0007]), not this one. The
//! transport is therefore a trait, curl is one implementation of it, and the
//! crate compiles with no process spawning at all so that portability is
//! checked rather than asserted.
//!
//! [ADR-0007]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0007-mobile-control-plane.md
//! [ADR-0016]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0016-real-providers-live-in-their-own-crates.md

#[cfg(feature = "curl")]
use std::io::Write as _;
#[cfg(feature = "curl")]
use std::process::{Command, Stdio};

use ephemeral_agent::AgentError;
use serde_json::Value;

/// Somewhere a request can be sent.
///
/// The whole seam is one method, because the transport decides nothing: it
/// carries bytes that [`crate::wire`] built and returns bytes that
/// [`crate::wire`] parses. Everything that could be wrong is on either side of
/// it.
///
/// An implementation must hold two properties that no test above it can check:
/// the credential must not reach an argument vector, a log, or an error
/// message; and every request must be bounded in time, or the wall-clock
/// ceiling on the loop above is defeated by a request that never returns.
pub trait Transport: Send + Sync {
    /// Sends one request and returns the parsed reply.
    ///
    /// # Errors
    ///
    /// [`AgentError::Unavailable`] when the transport itself cannot run,
    /// [`AgentError::Failed`] when the request fails, and
    /// [`AgentError::Unreadable`] when the reply is not JSON.
    fn send(&self, endpoint: &str, api_key: &str, request: &Value) -> Result<Value, AgentError>;
}

#[cfg(feature = "curl")]
/// How long one request may take before it is abandoned.
///
/// Generation is bounded on wall clock by the loop above this, but a request
/// that hangs forever would defeat that by never returning to be counted.
///
/// **Coupled to [`crate::wire::MAX_TOKENS`].** The reply is not streamed, so
/// nothing arrives until the model has finished writing all of it: a larger
/// token ceiling means a longer silence, not a longer trickle. Setting the two
/// independently is how a request times out having received zero bytes, which
/// is exactly what happened the first time the ceiling was raised. Raise one,
/// look at the other.
const TIMEOUT_SECONDS: u32 = 900;

/// The transport that spawns `curl`.
///
/// The desktop default. Unavailable on any platform that forbids spawning a
/// process, which is why it is a feature rather than the only option.
#[cfg(feature = "curl")]
#[derive(Debug, Clone, Copy, Default)]
pub struct Curl;

#[cfg(feature = "curl")]
impl Transport for Curl {
    fn send(&self, endpoint: &str, api_key: &str, request: &Value) -> Result<Value, AgentError> {
        send(endpoint, api_key, request)
    }
}

/// The arguments handed to `curl`.
///
/// A pure function, so what is asked of the network is a test rather than a
/// claim — and so that a reader can see there is nothing sensitive in it.
#[cfg(feature = "curl")]
#[must_use]
pub fn arguments(endpoint: &str) -> Vec<String> {
    vec![
        // Read the rest of the configuration, including the credential, from
        // stdin. Nothing sensitive is an argument.
        "--config".to_owned(),
        "-".to_owned(),
        "--silent".to_owned(),
        // Without this, `--silent` also swallows the reason a request failed.
        "--show-error".to_owned(),
        // A non-2xx response should be a failure, not a body that happens to
        // contain an error.
        "--fail-with-body".to_owned(),
        "--max-time".to_owned(),
        TIMEOUT_SECONDS.to_string(),
        endpoint.to_owned(),
    ]
}

#[cfg(feature = "curl")]
/// The `curl` configuration carrying the credential and the body.
///
/// Kept separate from [`arguments`] and never logged. The API key is in here
/// and nowhere else.
fn configuration(api_key: &str, body: &str) -> String {
    // `--data-raw` rather than `--data`, so a body beginning with `@` is never
    // read as a filename. A model-influenced request body that could name a
    // local file would be a fine way to exfiltrate one.
    format!(
        "header = \"x-api-key: {api_key}\"\n\
         header = \"anthropic-version: {version}\"\n\
         header = \"content-type: application/json\"\n\
         data-raw = {body}\n",
        version = crate::wire::API_VERSION,
        body = quote(body),
    )
}

#[cfg(feature = "curl")]
/// Quotes a value for a `curl` configuration file.
///
/// `curl` reads a double-quoted value with backslash escapes. A request body is
/// JSON and therefore full of quotes, so getting this wrong would corrupt every
/// request rather than fail loudly.
fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');

    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }

    quoted.push('"');
    quoted
}

#[cfg(feature = "curl")]
/// Sends one request and returns the parsed response.
///
/// # Errors
///
/// [`AgentError::Unavailable`] if `curl` is not there, and
/// [`AgentError::Failed`] if the request fails or the reply is not JSON.
pub fn send(endpoint: &str, api_key: &str, request: &Value) -> Result<Value, AgentError> {
    let args = arguments(endpoint);

    let mut child = Command::new("curl")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AgentError::Unavailable {
            provider: crate::NAME.to_owned(),
            reason: format!(
                "curl could not be run ({error}). Ephemeral uses it to reach the API; \
                 install it, or use `--provider mock`."
            ),
        })?;

    let configuration = configuration(api_key, &request.to_string());

    child
        .stdin
        .take()
        .ok_or_else(|| failed("curl's input could not be written to"))?
        .write_all(configuration.as_bytes())
        .map_err(|error| failed(&format!("the request could not be sent: {error}")))?;

    let output = child
        .wait_with_output()
        .map_err(|error| failed(&format!("curl did not finish: {error}")))?;

    let body = String::from_utf8_lossy(&output.stdout).into_owned();

    if !output.status.success() {
        // The body of a failed request is the API's own error, which says more
        // than curl's exit code. stderr covers the cases where there is no body.
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(failed(&describe_failure(&body, stderr.trim())));
    }

    serde_json::from_str(&body).map_err(|error| AgentError::Unreadable {
        provider: crate::NAME.to_owned(),
        reason: format!("the API's reply was not JSON: {error}"),
        raw: body,
    })
}

#[cfg(feature = "curl")]
/// What to tell somebody about a failed request.
///
/// Prefers the API's own message, which is almost always more useful than a
/// transport error — "your credit balance is too low" beats "exit status 22".
#[must_use]
fn describe_failure(body: &str, stderr: &str) -> String {
    let from_api = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });

    match from_api {
        Some(message) => message,
        None if !stderr.is_empty() => stderr.to_owned(),
        None if !body.is_empty() => body.to_owned(),
        None => "the request failed and said nothing about why".to_owned(),
    }
}

#[cfg(feature = "curl")]
/// A transport failure.
fn failed(reason: &str) -> AgentError {
    AgentError::Failed {
        provider: crate::NAME.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(all(test, feature = "curl"))]
mod tests {
    use super::*;
    use serde_json::json;

    /// The property that makes a recorded command safe to log or show.
    #[test]
    fn no_secret_reaches_the_argument_vector() {
        let args = arguments(crate::wire::ENDPOINT);
        let flattened = args.join(" ");

        assert!(!flattened.contains("sk-"), "{flattened}");
        assert!(!flattened.contains("x-api-key"), "{flattened}");
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--config" && pair[1] == "-"),
            "the credential must come in on stdin: {args:?}"
        );
    }

    /// A request that hangs forever would defeat the wall-clock ceiling above
    /// it by never returning to be counted.
    #[test]
    fn every_request_is_bounded_in_time() {
        let args = arguments(crate::wire::ENDPOINT);

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--max-time" && pair[1] == TIMEOUT_SECONDS.to_string()),
            "{args:?}"
        );
    }

    /// A non-2xx response must be a failure, not a body that happens to contain
    /// an error and would otherwise be parsed as a plan.
    #[test]
    fn an_error_response_is_a_failure_rather_than_a_body() {
        assert!(
            arguments(crate::wire::ENDPOINT)
                .iter()
                .any(|arg| arg == "--fail-with-body")
        );
    }

    /// A request body is JSON and therefore full of quotes. Getting this wrong
    /// would corrupt every request rather than fail loudly.
    #[test]
    fn a_json_body_survives_quoting() {
        let body = json!({ "text": "she said \"hi\"\nand left\\" }).to_string();
        let configuration = configuration("sk-test", &body);

        let quoted = configuration
            .lines()
            .find_map(|line| line.strip_prefix("data-raw = "))
            .expect("a body line");

        // Unquote it the way curl does — one left-to-right pass. A sequence of
        // `replace` calls gets this wrong: it turns the `\\n` produced by
        // escaping a literal backslash-then-n into a newline.
        let inner = quoted
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("a quoted value");

        let mut unquoted = String::new();
        let mut characters = inner.chars();
        while let Some(character) = characters.next() {
            if character != '\\' {
                unquoted.push(character);
                continue;
            }
            match characters.next() {
                Some('n') => unquoted.push('\n'),
                Some('r') => unquoted.push('\r'),
                Some('t') => unquoted.push('\t'),
                Some(escaped) => unquoted.push(escaped),
                None => unquoted.push('\\'),
            }
        }

        assert_eq!(unquoted, body);
    }

    /// A body beginning with `@` must never be read as a filename. That would
    /// be a fine way to exfiltrate a local file.
    #[test]
    fn a_body_is_never_read_as_a_filename() {
        let configuration = configuration("sk-test", "@/etc/passwd");

        assert!(configuration.contains("data-raw"), "{configuration}");
        assert!(!configuration.contains("\ndata = "), "{configuration}");
    }

    #[test]
    fn the_credential_and_the_version_travel_in_the_configuration() {
        let configuration = configuration("sk-test-value", "{}");

        assert!(configuration.contains("x-api-key: sk-test-value"));
        assert!(configuration.contains(crate::wire::API_VERSION));
    }

    /// "Your credit balance is too low" beats "exit status 22".
    #[test]
    fn a_failure_prefers_the_apis_own_message() {
        let body = json!({
            "type": "error",
            "error": { "type": "invalid_request_error", "message": "credit balance is too low" },
        })
        .to_string();

        assert_eq!(
            describe_failure(&body, "curl: (22) The requested URL returned error: 400"),
            "credit balance is too low"
        );
    }

    /// When there is no API message, whatever there is beats nothing.
    #[test]
    fn a_failure_with_no_api_message_falls_back_rather_than_going_quiet() {
        assert_eq!(
            describe_failure("", "curl: (6) Could not resolve host"),
            "curl: (6) Could not resolve host"
        );
        assert_eq!(describe_failure("upstream broke", ""), "upstream broke");
        assert!(!describe_failure("", "").is_empty());
    }
}
