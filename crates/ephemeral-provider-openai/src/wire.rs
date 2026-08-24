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

use ephemeral_agent::Model;
use ephemeral_agent::{
    AgentError,
    plan::{Plan, SourceFile},
};
use serde_json::{Value, json};

/// The base URL used unless one is named.
pub const BASE_URL: &str = "https://api.openai.com/v1";

/// The path appended to a base URL.
pub const PATH: &str = "/chat/completions";

/// Where this API lists the models it has.
pub const MODELS_PATH: &str = "/models";

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
/// The default, which is not the only possible value.
///
/// It was a constant, and that decided which models Ephemeral could use. A
/// model whose context window is smaller than this refuses the request outright
/// — Groq's `qwen3.6-27b` answers "`max_completion_tokens` must be less than or
/// equal to 16384" — and no amount of configuring anything could get past it.
/// A number that picks your models for you is the same mistake as a provider
/// that picks your vendor.
pub const DEFAULT_MAX_TOKENS: u32 = 32_000;

/// The default has to leave room for reasoning *and* an application, because
/// both are spent from it. Checked at compile time rather than by a test, since
/// it is a fact about a constant and not about behaviour. Somebody who lowers
/// it for a smaller model is making a trade knowingly; the default should not
/// make it for them.
const _: () = assert!(
    DEFAULT_MAX_TOKENS >= 32_000,
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

/// Where a service lists what it has, for a base URL of `base`.
#[must_use]
pub fn models_endpoint_from(base: &str) -> String {
    format!("{}{MODELS_PATH}", base.trim_end_matches('/'))
}

/// The models in a listing reply.
///
/// `{"data":[{"id":…}]}` is the whole of what this format promises. Everything
/// past that is a service being helpful in its own way, and is read when it is
/// there rather than required:
///
/// - `name` — Groq's readable label. Plain OpenAI has none, so the id is shown.
/// - `output_modalities` — what the model produces. A service that says a model
///   emits speech or a transcription is telling you it cannot write an
///   application, and offering it in a picker is offering a choice that fails
///   later, in a way nobody could have predicted from the name. Groq lists
///   thirteen models and three of them are like this.
///
/// A model that says nothing about its modalities is kept. Silence is not a
/// claim, and filtering on an absent field would empty the list for every
/// service that does not publish one.
#[must_use]
pub fn models_from(response: &Value) -> Vec<Model> {
    response["data"]
        .as_array()
        .map(|listed| listed.iter().filter_map(one_model).collect())
        .unwrap_or_default()
}

/// What a service said went wrong, when its reply is a refusal and not a listing.
///
/// Kept apart from [`models_from`], which stays total on purpose: as a *parser*,
/// answering "no models" for something that is not a listing is right, and it is
/// what stops untrusted input from panicking.
///
/// The bug that made this necessary was one level up. A caller that parsed zero
/// models and reported success turned a rejected key into "Reached it. 0
/// models." in green — on the one control whose entire job is telling
/// "configured" from "working". A rack phone photographed it saying exactly
/// that, a minute before generation failed for the same reason.
pub fn refusal_from(response: &Value) -> Option<String> {
    let error = response.get("error")?;

    // `{"error": {"message": …}}` is the shape OpenAI and everything that
    // copied it use. A bare string is not that shape and is still a refusal.
    let said = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .unwrap_or("it refused, and did not say why")
        .trim();

    Some(said.to_owned())
}

fn one_model(model: &Value) -> Option<Model> {
    let id = model["id"].as_str()?;

    if let Some(produces) = model["output_modalities"].as_array()
        && !produces.iter().any(|kind| kind.as_str() == Some("text"))
    {
        return None;
    }

    // `max_completion_tokens` when the service publishes it, the context window
    // otherwise: the second is an upper bound on the first and a better guess
    // than nothing. Plain OpenAI publishes neither, so this is usually absent.
    let ceiling = model["max_completion_tokens"]
        .as_u64()
        .or_else(|| model["context_window"].as_u64())
        .and_then(|value| u32::try_from(value).ok());

    let listed = match model["name"].as_str() {
        Some(name) if !name.is_empty() => Model::called(id, name),
        _ => Model::named(id),
    };

    Some(listed.holding(ceiling))
}

/// The request that asks for a plan.
#[must_use]
pub fn plan_request(model: &str, ceiling: Ceiling, tokens: u32, intent: &str) -> Value {
    body(
        model,
        ceiling,
        tokens,
        &ephemeral_agent::dialogue::plan_prompt(intent),
    )
}

/// The request that asks for the application itself.
#[must_use]
pub fn generate_request(model: &str, ceiling: Ceiling, tokens: u32, plan: &Plan) -> Value {
    body(
        model,
        ceiling,
        tokens,
        &ephemeral_agent::dialogue::generate_prompt(plan),
    )
}

/// The request that asks for a fix.
#[must_use]
pub fn repair_request(
    model: &str,
    ceiling: Ceiling,
    tokens: u32,
    files: &[SourceFile],
    failure: &str,
) -> Value {
    body(
        model,
        ceiling,
        tokens,
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
fn body(model: &str, ceiling: Ceiling, tokens: u32, prompt: &str) -> Value {
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
        object.insert(ceiling.field().to_owned(), json!(tokens));
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
            plan_request(
                DEFAULT_MODEL,
                Ceiling::Current,
                DEFAULT_MAX_TOKENS,
                "compare two CSV files",
            ),
            generate_request(DEFAULT_MODEL, Ceiling::Current, DEFAULT_MAX_TOKENS, &plan),
            repair_request(
                DEFAULT_MODEL,
                Ceiling::Current,
                DEFAULT_MAX_TOKENS,
                &[],
                "boom",
            ),
        ] {
            assert_eq!(request["max_completion_tokens"], DEFAULT_MAX_TOKENS);
            assert_eq!(request["model"], DEFAULT_MODEL);
        }
    }

    /// The dialect a server speaks decides the field name, and the ceiling has
    /// to arrive under the name that server reads — one it does not recognise
    /// is one it ignores.
    #[test]
    fn the_older_dialect_bounds_its_response_too() {
        let request = plan_request("qwen2.5-coder", Ceiling::Legacy, DEFAULT_MAX_TOKENS, "x");

        assert_eq!(request["max_tokens"], DEFAULT_MAX_TOKENS);
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
        let request = plan_request(DEFAULT_MODEL, Ceiling::Current, DEFAULT_MAX_TOKENS, "x");
        let messages = request["messages"].as_array().expect("messages");

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], ephemeral_agent::dialogue::SYSTEM);
        assert_eq!(messages[1]["role"], "user");
    }

    /// The two prompts are the shared ones, so a request built here asks for
    /// exactly what a request built by any other provider asks for.
    #[test]
    fn the_prompts_are_the_shared_ones() {
        let request = generate_request(
            DEFAULT_MODEL,
            Ceiling::Current,
            DEFAULT_MAX_TOKENS,
            &a_plan(),
        );
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
    /// A listing, read the way Groq actually answers one.
    ///
    /// Recorded from a real reply rather than imagined: `name`, `context_window`
    /// and `max_completion_tokens` are Groq's additions to a format that
    /// promises only `id`, and the modality fields are what separate a model
    /// that can write an application from one that transcribes audio.
    #[test]
    fn a_listing_keeps_what_can_write_an_application() {
        let reply = json!({
            "object": "list",
            "data": [
                {
                    "id": "openai/gpt-oss-120b",
                    "name": "GPT OSS 120B",
                    "context_window": 131_072,
                    "max_completion_tokens": 65536,
                    "output_modalities": ["text"]
                },
                {
                    "id": "whisper-large-v3",
                    "name": "Whisper",
                    "output_modalities": ["transcription"]
                },
                {
                    "id": "canopylabs/orpheus-v1-english",
                    "output_modalities": ["speech"]
                },
                { "id": "gpt-5" }
            ]
        });

        let listed = models_from(&reply);

        assert_eq!(
            listed
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["openai/gpt-oss-120b", "gpt-5"],
            "a model that cannot emit text cannot write an application"
        );

        assert_eq!(listed[0].name, "GPT OSS 120B");
        assert_eq!(
            listed[0].ceiling,
            Some(65536),
            "the reply ceiling, preferred over the whole context window"
        );

        // Plain OpenAI publishes neither a name nor a size. Silence is not a
        // claim, so the model is kept and shown by its id.
        assert_eq!(listed[1].name, "gpt-5");
        assert_eq!(listed[1].ceiling, None);
    }

    /// The context window stands in when a service does not publish a reply
    /// ceiling: it is an upper bound on one, and a bound is better than nothing
    /// for the setting most likely to be wrong.
    #[test]
    fn the_context_window_stands_in_for_an_unpublished_ceiling() {
        let reply = json!({ "data": [{ "id": "small", "context_window": 4096 }] });

        assert_eq!(models_from(&reply)[0].ceiling, Some(4096));
    }

    /// A refusal is recognised as one, in the service's own words.
    ///
    /// The listing endpoint answering `{"error": …}` means the key was rejected
    /// or the endpoint is not what somebody thought. Reading that as an empty
    /// listing is how a rejection came to be reported as a success.
    #[test]
    fn a_refusal_is_told_apart_from_an_empty_listing() {
        let rejected = json!({
            "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" }
        });
        assert_eq!(
            refusal_from(&rejected).as_deref(),
            Some("Incorrect API key provided")
        );

        // A service that answers with a listing is not refusing, however short
        // the listing is. Zero models is a fact about the account, not an error.
        assert_eq!(refusal_from(&json!({ "data": [] })), None);
        assert_eq!(refusal_from(&json!({ "data": [{ "id": "gpt-5" }] })), None);
    }

    /// Even a refusal that says nothing useful is still a refusal.
    #[test]
    fn a_refusal_with_no_message_is_still_a_refusal() {
        assert!(refusal_from(&json!({ "error": "nope" })).is_some());
        assert!(refusal_from(&json!({ "error": {} })).is_some());
    }

    /// A reply that is not a listing yields nothing rather than a panic. This
    /// is untrusted input from whatever the base URL points at.
    #[test]
    fn a_reply_that_is_not_a_listing_is_no_models() {
        assert!(models_from(&json!({ "error": "nope" })).is_empty());
        assert!(models_from(&json!({ "data": "not an array" })).is_empty());
        assert!(models_from(&json!({ "data": [{ "no": "id" }] })).is_empty());
    }

    /// The ceiling is a value the caller supplies, and it has to reach the
    /// request under whichever name the service reads. This is the whole of
    /// what made a 16k model usable.
    #[test]
    fn the_ceiling_the_caller_asks_for_is_the_one_that_is_sent() {
        let current = plan_request("m", Ceiling::Current, 7000, "x");
        assert_eq!(current["max_completion_tokens"], 7000);

        let legacy = plan_request("m", Ceiling::Legacy, 7000, "x");
        assert_eq!(legacy["max_tokens"], 7000);
    }
}
