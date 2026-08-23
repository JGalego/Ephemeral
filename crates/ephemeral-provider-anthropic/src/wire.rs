//! What goes out, and what comes back — all of it pure.
//!
//! Everything in this module is a function from data to data. No process is
//! spawned, no socket opened, no credential read. That is what makes the
//! interesting half of a provider testable in CI, which is forbidden from
//! making a live model call ([ADR-0008]).
//!
//! The division is deliberate: prompt construction and response parsing are
//! where the bugs are, and [`ephemeral_agent::transport`] — which is the only part CI
//! cannot exercise — is about thirty lines that hand a string to `curl`.
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md

use ephemeral_agent::{
    AgentError,
    plan::{Plan, SourceFile},
};
use serde_json::{Value, json};

/// The API version header value this provider is written against.
pub const API_VERSION: &str = "2023-06-01";

/// Where requests go.
pub const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// Anthropic's own base URL, before the path this provider appends.
pub const BASE_URL: &str = "https://api.anthropic.com";

/// The path a request is sent to, under whatever base URL is in use.
pub const PATH: &str = "/v1/messages";

/// Where requests go, for a service whose base URL is `base`.
///
/// A gateway, a proxy, or a company's own front door for this API. Base URLs
/// are written both ways by everyone who publishes one, so a trailing slash is
/// trimmed rather than reported.
#[must_use]
pub fn endpoint_from(base: &str) -> String {
    format!("{}{PATH}", base.trim_end_matches('/'))
}

/// The model used unless one is named.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5";

/// The ceiling on one response.
///
/// Bounded because an unbounded response is an unbounded bill — but the first
/// value chosen, 16,000, was too small and every real generation hit it. A
/// model's own reasoning is spent from this same budget before a single line of
/// the application is written, so a ceiling sized for the output alone leaves
/// nothing to write with.
///
/// Truncation is not a graceful degradation: a half-written JSON object is
/// unreadable, so the whole attempt is wasted. That asymmetry argues for a
/// generous ceiling, since the cost of one that is too high is paid only when a
/// response genuinely needs the room.
///
/// It is not free, though. The reply is not streamed, so a bigger ceiling means
/// a longer wait before *anything* arrives — see
/// [`ephemeral_agent::transport`]'s timeout, which has to be large enough for a reply
/// this size. The two move together.
pub(crate) const MAX_TOKENS: u32 = 32_000;

/// The ceiling has to leave room for reasoning *and* an application, because
/// both are spent from it. Checked at compile time rather than by a test, since
/// it is a fact about a constant and not about behaviour.
const _: () = assert!(
    MAX_TOKENS >= 32_000,
    "a whole application plus the model's own reasoning does not fit"
);

/// The request that asks for a plan.
#[must_use]
pub fn plan_request(model: &str, intent: &str) -> Value {
    body(model, &ephemeral_agent::dialogue::plan_prompt(intent))
}

/// The request that asks for the application itself.
#[must_use]
pub fn generate_request(model: &str, plan: &Plan) -> Value {
    body(model, &ephemeral_agent::dialogue::generate_prompt(plan))
}

/// The request that asks for a fix.
#[must_use]
pub fn repair_request(model: &str, files: &[SourceFile], failure: &str) -> Value {
    body(
        model,
        &ephemeral_agent::dialogue::repair_prompt(files, failure),
    )
}

/// The headers this API needs, credential included.
///
/// The provider owns these, not the transport: Anthropic wants `x-api-key` and
/// its own API version, an OpenAI-compatible service wants `Authorization:
/// Bearer`, and a transport that knew either secretly belonged to one provider.
#[must_use]
pub fn headers(api_key: &str) -> Vec<(String, String)> {
    vec![
        ("x-api-key".to_owned(), api_key.to_owned()),
        ("anthropic-version".to_owned(), API_VERSION.to_owned()),
        ("content-type".to_owned(), "application/json".to_owned()),
    ]
}

/// The common envelope.
fn body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": ephemeral_agent::dialogue::SYSTEM,
        "messages": [{ "role": "user", "content": prompt }],
    })
}

/// What a response cost.
#[must_use]
pub fn usage_from(response: &Value) -> ephemeral_agent::Usage {
    let usage = response.get("usage");
    let count = |field: &str| -> u64 {
        usage
            .and_then(|usage| usage.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };

    ephemeral_agent::Usage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
        // Deliberately zero. Prices change without notice, and a number
        // Ephemeral invented and presented as a cost would be worse than no
        // number at all. The spend ceiling is therefore only meaningful once a
        // caller supplies real pricing.
        cents: 0,
    }
}

/// The text a response carries, or an error naming what was wrong with it.
///
/// # Errors
///
/// [`AgentError::Unreadable`] with the raw response, which is the thing anybody
/// debugging this needs to see.
pub fn text_from(response: &Value) -> Result<String, AgentError> {
    // A refusal or a truncated response is not a parse failure, and saying so
    // is the difference between "the model would not do it" and "Ephemeral is
    // broken".
    if let Some(reason) = response.get("stop_reason").and_then(Value::as_str)
        && reason == "max_tokens"
    {
        return Err(ephemeral_agent::dialogue::unreadable(
            crate::NAME,
            "the model ran out of room before it finished, so the reply is incomplete. \
             Asking for something smaller usually fixes it; the whole application has to \
             arrive in one reply",
            response,
        ));
    }

    let text = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .find_map(|block| block.get("text").and_then(Value::as_str))
        })
        .ok_or_else(|| {
            ephemeral_agent::dialogue::unreadable(
                crate::NAME,
                "the response carried no text",
                response,
            )
        })?;

    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(text: &str) -> Value {
        json!({
            "content": [{ "type": "text", "text": text }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1200, "output_tokens": 400 },
        })
    }

    /// The system prompt has to describe the sandbox the code will actually run
    /// in, or the model writes something that cannot possibly work.
    #[test]
    fn every_request_bounds_its_response() {
        let plan = Plan {
            summary: "x".to_owned(),
            interface: ephemeral_core::manifest::AppInterface::Job,
            runtime: ephemeral_core::manifest::RuntimeKind::Docker,
            image: "python:3.12-slim".to_owned(),
            requests: Vec::new(),
        };

        for request in [
            plan_request(DEFAULT_MODEL, "compare two CSV files"),
            generate_request(DEFAULT_MODEL, &plan),
            repair_request(DEFAULT_MODEL, &[], "boom"),
        ] {
            assert_eq!(request["max_tokens"], MAX_TOKENS);
            assert_eq!(request["model"], DEFAULT_MODEL);
            assert!(request["system"].as_str().is_some_and(|s| !s.is_empty()));
        }
    }

    /// Build output is attacker-controlled. It is delimited and labelled as
    /// data, which is a mitigation rather than a guarantee.
    #[test]
    fn a_truncated_response_says_it_was_truncated() {
        let cut_off = json!({
            "content": [{ "type": "text", "text": "{\"files\": [" }],
            "stop_reason": "max_tokens",
        });

        let error = text_from(&cut_off).expect_err("a truncated response is unusable");
        let message = error.to_string();

        assert!(message.contains("ran out of room"), "{message}");
        assert!(
            message.contains("smaller"),
            "a truncation a user can do nothing about is not a useful error: {message}"
        );
    }

    #[test]
    fn a_response_with_no_text_is_unreadable_rather_than_empty() {
        assert!(text_from(&json!({ "content": [] })).is_err());
        assert!(text_from(&json!({})).is_err());
    }

    #[test]
    fn text_is_read_from_a_normal_response() {
        assert_eq!(text_from(&reply("hello")).expect("text"), "hello");
    }

    /// A price Ephemeral invented and presented as a cost would be worse than
    /// no number at all.
    #[test]
    fn token_counts_are_reported_and_a_price_is_not_invented() {
        let usage = usage_from(&reply("x"));

        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 400);
        assert_eq!(usage.cents, 0, "Ephemeral does not know the price");
    }
    /// The endpoint constant and the one built from the base URL have to be the
    /// same place, or pointing this at "Anthropic" explicitly would reach
    /// somewhere different from leaving it alone.
    #[test]
    fn the_default_endpoint_is_what_the_default_base_url_builds() {
        assert_eq!(endpoint_from(BASE_URL), ENDPOINT);
    }

    /// A gateway is a base URL, and the path this provider posts to is appended
    /// to it rather than assumed to be part of it.
    #[test]
    fn a_gateway_is_reached_at_the_path_this_provider_uses() {
        assert_eq!(
            endpoint_from("https://gateway.example.com/anthropic/"),
            "https://gateway.example.com/anthropic/v1/messages",
            "a trailing slash is trimmed rather than doubled"
        );
    }
}
