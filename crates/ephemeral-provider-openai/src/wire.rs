//! What goes out, and what comes back — all of it pure.
//!
//! The shape here is OpenAI's chat completions API, which is the closest thing
//! generation has to a lingua franca: OpenAI itself speaks it, and so do
//! Ollama, llama.cpp's server, LM Studio, vLLM and most hosted services that
//! came after it. One wire format therefore reaches both a hosted model and a
//! model on the user's own machine, which is why
//! [`ephemeral-provider-local`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-provider-local)
//! uses this module rather than owning a second copy of it ([ADR-0019]).
//!
//! Everything in this module is a function from data to data. No process is
//! spawned, no socket opened, no credential read. That is what makes the
//! interesting half of a provider testable in CI, which is forbidden from
//! making a live model call ([ADR-0008]).
//!
//! [ADR-0008]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0008-agent-provider-abstraction.md
//! [ADR-0019]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0019-openai-compatible-and-a-local-model.md

use ephemeral_agent::{
    AgentError,
    plan::{Plan, SourceFile},
};
use serde_json::{Value, json};

/// The base URL used unless one is named.
pub const BASE_URL: &str = "https://api.openai.com/v1";

/// The path appended to a base URL.
pub const PATH: &str = "/chat/completions";

/// The model used unless one is named.
pub const DEFAULT_MODEL: &str = "gpt-5";

/// The ceiling on one response.
///
/// The same number, and the same reasoning, as the Anthropic provider's: a
/// model's own reasoning is spent from this budget before a line of the
/// application is written, and a truncated reply is a wasted attempt rather
/// than a degraded one, because half a JSON object parses as nothing.
///
/// It is not free. The reply is not streamed, so a larger ceiling means a
/// longer silence before anything arrives at all — see the transport's
/// timeout, which has to be large enough for a reply this size. The two move
/// together.
pub(crate) const MAX_TOKENS: u32 = 32_000;

/// The ceiling has to leave room for reasoning *and* an application, because
/// both are spent from it. Checked at compile time rather than by a test, since
/// it is a fact about a constant and not about behaviour.
const _: () = assert!(
    MAX_TOKENS >= 32_000,
    "a whole application plus the model's own reasoning does not fit"
);

/// Which field carries the response ceiling.
///
/// The one place the format's dialects genuinely disagree. OpenAI renamed
/// `max_tokens` to `max_completion_tokens` when reasoning models arrived, and
/// its newer models reject the old name outright; the servers that copied the
/// format copied it before the rename, and a good many of them know only
/// `max_tokens`.
///
/// So it is a parameter rather than a constant. Sending both is not an option —
/// OpenAI rejects a request carrying the two — and sending neither would leave
/// every response unbounded, which is the one thing this must not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ceiling {
    /// `max_completion_tokens`, which is what OpenAI's own API takes now.
    Current,

    /// `max_tokens`, which is what most self-hosted servers take.
    Legacy,
}

impl Ceiling {
    /// The field name this dialect uses.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Current => "max_completion_tokens",
            Self::Legacy => "max_tokens",
        }
    }
}

/// Where requests go, for a service whose base URL is `base`.
///
/// Base URLs are written both ways by everyone who publishes one, so a trailing
/// slash is not an error worth reporting — it is just trimmed.
#[must_use]
pub fn endpoint_from(base: &str) -> String {
    format!("{}{PATH}", base.trim_end_matches('/'))
}

/// The request that asks for a plan.
#[must_use]
pub fn plan_request(model: &str, ceiling: Ceiling, intent: &str) -> Value {
    body(
        model,
        ceiling,
        &ephemeral_agent::dialogue::plan_prompt(intent),
    )
}

/// The request that asks for the application itself.
#[must_use]
pub fn generate_request(model: &str, ceiling: Ceiling, plan: &Plan) -> Value {
    body(
        model,
        ceiling,
        &ephemeral_agent::dialogue::generate_prompt(plan),
    )
}

/// The request that asks for a fix.
#[must_use]
pub fn repair_request(model: &str, ceiling: Ceiling, files: &[SourceFile], failure: &str) -> Value {
    body(
        model,
        ceiling,
        &ephemeral_agent::dialogue::repair_prompt(files, failure),
    )
}

/// The headers this API needs, with a credential if there is one.
///
/// `None` is not an oversight: a model on the loopback interface is reached
/// with no credential at all, and a provider that had to invent one to satisfy
/// this signature would end up sending something.
#[must_use]
pub fn headers(api_key: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];

    if let Some(key) = api_key {
        headers.push(("authorization".to_owned(), format!("Bearer {key}")));
    }

    headers
}

/// The common envelope.
///
/// The system instruction is a message with the `system` role rather than a
/// field of its own, which is the whole structural difference from Anthropic's
/// envelope. `system` is also the role every server that copied this format
/// understands, including the ones that predate OpenAI's `developer`.
fn body(model: &str, ceiling: Ceiling, prompt: &str) -> Value {
    let mut body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": ephemeral_agent::dialogue::SYSTEM },
            { "role": "user", "content": prompt },
        ],
    });

    // Inserted rather than written into the literal above, because which field
    // this is depends on the dialect and there is only ever one of it.
    if let Some(object) = body.as_object_mut() {
        object.insert(ceiling.field().to_owned(), json!(MAX_TOKENS));
    }

    body
}

/// What a response cost.
///
/// Token counts only. Prices change without notice, and a number Ephemeral
/// invented and presented as a cost would be worse than no number at all — so
/// `cents` stays zero here, exactly as it does for every other provider.
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
        input_tokens: count("prompt_tokens"),
        output_tokens: count("completion_tokens"),
        cents: 0,
    }
}

/// The text a response carries, or an error naming what was wrong with it.
///
/// Takes the provider's name because two providers share this module, and the
/// person reading the error needs to know which of them was being used.
///
/// # Errors
///
/// [`AgentError::Failed`] when the model declined, and
/// [`AgentError::Unreadable`] — carrying the raw response, which is the thing
/// anybody debugging this needs to see — for everything else.
pub fn text_from(provider: &str, response: &Value) -> Result<String, AgentError> {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| {
            ephemeral_agent::dialogue::unreadable(
                provider,
                "the response carried no choices",
                response,
            )
        })?;

    // A refusal or a truncated response is not a parse failure, and saying so
    // is the difference between "the model would not do it" and "Ephemeral is
    // broken".
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(ephemeral_agent::dialogue::unreadable(
            provider,
            "the model ran out of room before it finished, so the reply is incomplete. \
             Asking for something smaller usually fixes it; the whole application has to \
             arrive in one reply",
            response,
        ));
    }

    let message = choice.get("message").unwrap_or(&Value::Null);

    // A declined request has an answer, and it is not "Ephemeral could not read
    // this". The model's own words are the useful part.
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str)
        && !refusal.trim().is_empty()
    {
        return Err(AgentError::Failed {
            provider: provider.to_owned(),
            reason: format!("the model declined: {refusal}"),
        });
    }

    let text = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| {
            ephemeral_agent::dialogue::unreadable(
                provider,
                "the response carried no text",
                response,
            )
        })?;

    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "openai";

    fn reply(text: &str) -> Value {
        json!({
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 1200, "completion_tokens": 400 },
        })
    }

    fn a_plan() -> Plan {
        Plan {
            summary: "x".to_owned(),
            interface: ephemeral_core::manifest::AppInterface::Job,
            runtime: ephemeral_core::manifest::RuntimeKind::Docker,
            image: "python:3.12-slim".to_owned(),
            requests: Vec::new(),
        }
    }

    /// An unbounded response is an unbounded bill, and an unbounded wait.
    #[test]
    fn every_request_bounds_its_response() {
        let plan = a_plan();

        for request in [
            plan_request(DEFAULT_MODEL, Ceiling::Current, "compare two CSV files"),
            generate_request(DEFAULT_MODEL, Ceiling::Current, &plan),
            repair_request(DEFAULT_MODEL, Ceiling::Current, &[], "boom"),
        ] {
            assert_eq!(request["max_completion_tokens"], MAX_TOKENS);
            assert_eq!(request["model"], DEFAULT_MODEL);
        }
    }

    /// The dialect a server speaks decides the field name, and the ceiling has
    /// to arrive under the name that server reads — one it does not recognise
    /// is one it ignores.
    #[test]
    fn the_older_dialect_bounds_its_response_too() {
        let request = plan_request("qwen2.5-coder", Ceiling::Legacy, "x");

        assert_eq!(request["max_tokens"], MAX_TOKENS);
        assert!(
            request.get("max_completion_tokens").is_none(),
            "sending both is a request OpenAI rejects: {request}"
        );
    }

    /// The constraints a generated application must satisfy are in the system
    /// instruction. A request that lost it would ask for an application that
    /// cannot run in the sandbox.
    #[test]
    fn every_request_carries_the_system_instruction() {
        let request = plan_request(DEFAULT_MODEL, Ceiling::Current, "x");
        let messages = request["messages"].as_array().expect("messages");

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], ephemeral_agent::dialogue::SYSTEM);
        assert_eq!(messages[1]["role"], "user");
    }

    /// The two prompts are the shared ones, so a request built here asks for
    /// exactly what a request built by any other provider asks for.
    #[test]
    fn the_prompts_are_the_shared_ones() {
        let request = generate_request(DEFAULT_MODEL, Ceiling::Current, &a_plan());
        let sent = request["messages"][1]["content"]
            .as_str()
            .expect("a user message");

        assert_eq!(
            sent,
            ephemeral_agent::dialogue::generate_prompt(&a_plan()),
            "a provider that writes its own prompts drifts from the others"
        );
    }

    #[test]
    fn a_base_url_reaches_the_same_endpoint_written_either_way() {
        assert_eq!(
            endpoint_from("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint_from("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    /// The credential goes in a header and nowhere else, and a provider with no
    /// credential sends no header rather than an empty one.
    #[test]
    fn a_credential_travels_as_a_bearer_header_when_there_is_one() {
        let with = headers(Some("sk-test-value"));
        assert!(
            with.iter()
                .any(|(name, value)| name == "authorization" && value == "Bearer sk-test-value")
        );

        let without = headers(None);
        assert!(
            without.iter().all(|(name, _)| name != "authorization"),
            "no credential means no header: {without:?}"
        );
        assert!(without.iter().any(|(name, _)| name == "content-type"));
    }

    /// A truncated reply is unusable, and saying why is the difference between
    /// a person shortening their request and a person filing a bug.
    #[test]
    fn a_truncated_response_says_it_was_truncated() {
        let cut_off = json!({
            "choices": [{
                "message": { "content": "{\"files\": [" },
                "finish_reason": "length",
            }],
        });

        let error = text_from(PROVIDER, &cut_off).expect_err("a truncated response is unusable");
        let message = error.to_string();

        assert!(message.contains("ran out of room"), "{message}");
        assert!(message.contains("smaller"), "{message}");
    }

    /// "The model would not do it" is not "Ephemeral is broken", and the model's
    /// own words say which of the two happened.
    #[test]
    fn a_refusal_is_reported_as_one() {
        let declined = json!({
            "choices": [{
                "message": { "content": null, "refusal": "I can't help with that." },
                "finish_reason": "stop",
            }],
        });

        let error = text_from(PROVIDER, &declined).expect_err("a refusal is not a reply");

        assert!(
            matches!(&error, AgentError::Failed { reason, .. } if reason.contains("can't help")),
            "{error:?}"
        );
    }

    #[test]
    fn a_response_with_no_text_is_unreadable_rather_than_empty() {
        assert!(text_from(PROVIDER, &json!({ "choices": [] })).is_err());
        assert!(text_from(PROVIDER, &json!({})).is_err());
        assert!(text_from(PROVIDER, &reply("   ")).is_err());
    }

    #[test]
    fn text_is_read_from_a_normal_response() {
        assert_eq!(text_from(PROVIDER, &reply("hello")).expect("text"), "hello");
    }

    /// The error names the provider that produced it, because two providers
    /// share this module and "which one was I using" is the first question.
    #[test]
    fn an_unreadable_reply_names_the_provider_that_produced_it() {
        let error = text_from("local", &json!({})).expect_err("nothing to read");

        assert!(error.to_string().contains("local"), "{error}");
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

    /// A server that reports no usage at all is normal on the loopback
    /// interface, and is not a failure.
    #[test]
    fn a_response_without_usage_counts_nothing_rather_than_failing() {
        let usage = usage_from(&json!({ "choices": [] }));

        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }
}
