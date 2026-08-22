//! # OpenAI-compatible provider
//!
//! A second real [`AgentProvider`], and the more useful of the two shapes: the
//! chat completions format it speaks is the one OpenAI publishes *and* the one
//! Ollama, llama.cpp, LM Studio, vLLM and most hosted services accept. One
//! implementation therefore reaches OpenAI itself, anything that copied it, and
//! — through [`ephemeral-provider-local`](https://github.com/JGalego/Ephemeral/tree/main/crates/ephemeral-provider-local)
//! — a model on the user's own machine ([ADR-0019]).
//!
//! Point it somewhere else with `OPENAI_BASE_URL`; name the model with
//! `OPENAI_MODEL`. Neither is guesswork on Ephemeral's part: a service that
//! speaks this format publishes both.
//!
//! ## What is tested and what is not
//!
//! Prompt construction, request bodies, response parsing, capability
//! translation and error mapping are pure functions in [`wire`], and all of
//! them are tested. [`ephemeral_agent::transport`] hands a string to `curl` and
//! is the only part CI cannot exercise, because doing so would need a
//! credential ([ADR-0016]).
//!
//! ## Nothing here is trusted
//!
//! A model's reply is parsed as data and validated. A reply that does not parse
//! is an error rather than a best-effort guess; a plan asking for something it
//! will not explain is refused; a file path that escapes the application is
//! refused rather than normalised. None of that depends on the model
//! cooperating, which is why it is here rather than in the prompt.
//!
//! [ADR-0016]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0016-real-providers-live-in-their-own-crates.md
//! [ADR-0019]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0019-openai-compatible-and-a-local-model.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod wire;

use ephemeral_agent::dialogue;
use ephemeral_agent::{
    AgentError, AgentProvider, Attempt,
    plan::{GeneratedApp, Plan, RepairAttempt, SourceFile},
    transport::{HttpRequest, Transport},
};

use wire::Ceiling;

/// What this provider is called, in the interface and the audit record.
pub const NAME: &str = "openai";

/// The environment variable the credential is read from.
pub const API_KEY_VARIABLE: &str = "OPENAI_API_KEY";

/// The environment variable that points this somewhere other than OpenAI.
pub const BASE_URL_VARIABLE: &str = "OPENAI_BASE_URL";

/// The environment variable that names the model.
pub const MODEL_VARIABLE: &str = "OPENAI_MODEL";

/// The environment variable that names the field carrying the response ceiling.
///
/// An escape hatch for a service that speaks this format but predates OpenAI
/// renaming `max_tokens`. Set it to `max_tokens` for one of those. It exists
/// because the alternative — guessing from the base URL — would be Ephemeral
/// deciding, quietly and sometimes wrongly, how a response gets bounded.
pub const CEILING_VARIABLE: &str = "OPENAI_TOKEN_CEILING_FIELD";

/// Generates applications with any service that speaks OpenAI's chat
/// completions format.
pub struct OpenAiProvider {
    api_key: Option<String>,
    model: String,
    endpoint: String,
    /// The dialect, or the value somebody set that named neither.
    ///
    /// Held as a failure rather than resolved to a default, because a ceiling
    /// silently not applied is an unbounded reply, and an unbounded reply is
    /// the one thing every provider here is built to prevent.
    ceiling: Result<Ceiling, String>,
    transport: Box<dyn Transport>,
}

#[cfg(feature = "curl")]
impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// A trimmed environment variable, if it is set to anything.
fn from_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

impl OpenAiProvider {
    /// A provider configured from the environment, sending through `curl`.
    ///
    /// Read once, at construction, so nothing is fetched again per request —
    /// and the credential is held as an `Option` so that "no credential" is a
    /// state this type can describe rather than something discovered
    /// mid-generation.
    #[cfg(feature = "curl")]
    #[must_use]
    pub fn new() -> Self {
        Self::with_transport(Box::new(ephemeral_agent::transport::Curl::for_provider(
            NAME,
        )))
    }

    /// The same, sending through a transport the caller supplies.
    ///
    /// This is the constructor a phone would use. Spawning `curl` is impossible
    /// on iOS, and reading a credential from an environment variable is not how
    /// a mobile application holds a secret either — both are desktop answers,
    /// and both are supplied from outside rather than assumed here.
    #[must_use]
    pub fn with_transport(transport: Box<dyn Transport>) -> Self {
        let base = from_environment(BASE_URL_VARIABLE).unwrap_or_else(|| wire::BASE_URL.to_owned());

        Self {
            api_key: from_environment(API_KEY_VARIABLE),
            model: from_environment(MODEL_VARIABLE)
                .unwrap_or_else(|| wire::DEFAULT_MODEL.to_owned()),
            endpoint: wire::endpoint_from(&base),
            ceiling: from_environment(CEILING_VARIABLE).map_or(Ok(Ceiling::Current), |field| {
                match field.as_str() {
                    "max_completion_tokens" => Ok(Ceiling::Current),
                    "max_tokens" => Ok(Ceiling::Legacy),
                    other => Err(other.to_owned()),
                }
            }),
            transport,
        }
    }

    /// The same, with a credential the caller already holds.
    ///
    /// For a platform whose secret store is not the environment — a Keychain, a
    /// Keystore, a Credential Manager.
    #[must_use]
    pub fn with_credential(mut self, api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        self.api_key = Some(api_key).filter(|key| !key.trim().is_empty());
        self
    }

    /// The same, with a particular model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The same, pointed at a service whose base URL is `base`.
    #[must_use]
    pub fn with_base_url(mut self, base: &str) -> Self {
        self.endpoint = wire::endpoint_from(base);
        self
    }

    /// The same, speaking a particular dialect of the response ceiling.
    ///
    /// Public because the servers that copied this format copied it before
    /// OpenAI renamed the field, and the caller is the one that knows which of
    /// them it is talking to.
    #[must_use]
    pub fn with_ceiling(mut self, ceiling: Ceiling) -> Self {
        self.ceiling = Ok(ceiling);
        self
    }

    /// Where this provider sends its requests.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The dialect to send, or the reason it is not known.
    fn ceiling(&self) -> Result<Ceiling, AgentError> {
        self.ceiling
            .as_ref()
            .copied()
            .map_err(|value| AgentError::Unavailable {
                provider: NAME.to_owned(),
                reason: format!(
                    "{CEILING_VARIABLE} is set to {value:?}, which is neither \
                     `max_completion_tokens` nor `max_tokens`. A response ceiling sent \
                     under a name the service does not read is no ceiling at all."
                ),
            })
    }

    /// The credential, or the reason there is not one.
    fn key(&self) -> Result<&str, AgentError> {
        self.api_key
            .as_deref()
            .ok_or_else(|| AgentError::Unavailable {
                provider: NAME.to_owned(),
                reason: format!(
                    "no API key. Set {API_KEY_VARIABLE}, or use `--provider local` to generate \
                     with a model on this machine, or `--provider mock` to build the example \
                     application without either."
                ),
            })
    }

    /// One round trip: send, read the text, read the JSON in it.
    fn ask(
        &self,
        request: &serde_json::Value,
    ) -> Result<(serde_json::Value, ephemeral_agent::Usage), AgentError> {
        let response = self.transport.send(&HttpRequest {
            endpoint: &self.endpoint,
            headers: wire::headers(Some(self.key()?)),
            body: request,
        })?;
        let usage = wire::usage_from(&response);
        let text = wire::text_from(NAME, &response)?;

        Ok((dialogue::json_from(NAME, &text)?, usage))
    }
}

impl AgentProvider for OpenAiProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn availability(&self) -> Result<(), AgentError> {
        // Deliberately does not reach the network: an unavailable provider is a
        // diagnosis `ephemeral doctor` reports, not something discovered by
        // trying.
        self.key()?;
        self.ceiling().map(|_| ())
    }

    fn plan(&self, intent: &str) -> Result<Attempt<Plan>, AgentError> {
        let request = wire::plan_request(&self.model, self.ceiling()?, intent);
        let (value, usage) = self.ask(&request)?;

        Ok(Attempt::new(dialogue::plan_from(NAME, &value)?, usage))
    }

    fn generate(&self, plan: &Plan) -> Result<Attempt<GeneratedApp>, AgentError> {
        let request = wire::generate_request(&self.model, self.ceiling()?, plan);
        let (value, usage) = self.ask(&request)?;

        Ok(Attempt::new(dialogue::app_from(NAME, &value, plan)?, usage))
    }

    fn repair(
        &self,
        _app: &GeneratedApp,
        files: &[SourceFile],
        failure: &str,
    ) -> Result<Attempt<RepairAttempt>, AgentError> {
        let request = wire::repair_request(&self.model, self.ceiling()?, files, failure);
        let (value, usage) = self.ask(&request)?;

        Ok(Attempt::new(dialogue::repair_from(NAME, &value)?, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one request looked like by the time it reached the transport.
    type Sent = (String, Vec<(String, String)>, serde_json::Value);

    /// A transport that answers from a script instead of the network, and
    /// records what it was given.
    ///
    /// This is what the trait bought: the provider can be driven end to end in
    /// CI, with no credential, no `curl`, and no network — and what went out
    /// can be asserted on, which is the half that matters for a credential.
    struct Scripted {
        reply: serde_json::Value,
        seen: std::sync::Arc<std::sync::Mutex<Vec<Sent>>>,
    }

    impl Scripted {
        /// The transport, and the log it writes to.
        fn answering(
            reply: serde_json::Value,
        ) -> (
            Box<dyn Transport>,
            std::sync::Arc<std::sync::Mutex<Vec<Sent>>>,
        ) {
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let transport = Self {
                reply,
                seen: std::sync::Arc::clone(&seen),
            };

            (Box::new(transport), seen)
        }
    }

    impl Transport for Scripted {
        fn send(&self, request: &HttpRequest<'_>) -> Result<serde_json::Value, AgentError> {
            if let Ok(mut seen) = self.seen.lock() {
                seen.push((
                    request.endpoint.to_owned(),
                    request.headers.clone(),
                    request.body.clone(),
                ));
            }

            Ok(self.reply.clone())
        }
    }

    /// A transport that must never be reached.
    struct Forbidden;

    impl Transport for Forbidden {
        fn send(&self, _request: &HttpRequest<'_>) -> Result<serde_json::Value, AgentError> {
            panic!("the transport was used without a credential");
        }
    }

    /// A reply carrying `text` the way the API frames it.
    fn replying_with(text: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 22 },
        })
    }

    /// Built by hand rather than from the environment, because a test that
    /// reads the environment passes or fails depending on the machine it runs
    /// on.
    fn provider(transport: Box<dyn Transport>) -> OpenAiProvider {
        OpenAiProvider {
            api_key: None,
            model: wire::DEFAULT_MODEL.to_owned(),
            endpoint: wire::endpoint_from(wire::BASE_URL),
            ceiling: Ok(Ceiling::Current),
            transport,
        }
    }

    fn without_a_key() -> OpenAiProvider {
        provider(Box::new(Forbidden))
    }

    const A_PLAN: &str = r#"{"name":"CSV Comparator","summary":"compares two CSV files",
        "runtime":"docker","image":"python:3.12-slim",
        "interface":"command_line","permissions":[]}"#;

    /// A missing credential is a diagnosis with a remedy, not a failure
    /// somebody discovers halfway through a generation run.
    #[test]
    fn a_missing_credential_is_reported_before_anything_is_attempted() {
        let error = without_a_key()
            .availability()
            .expect_err("no key, no provider");
        let message = error.to_string();

        assert!(message.contains(API_KEY_VARIABLE), "{message}");
        assert!(
            message.contains("local"),
            "the offline way forward is the one worth naming first: {message}"
        );
        assert!(message.contains("mock"), "{message}");
    }

    /// Nothing is sent without a credential, so a missing one cannot become a
    /// mysterious network error.
    #[test]
    fn nothing_is_attempted_without_a_credential() {
        assert!(matches!(
            without_a_key().plan("x").unwrap_err(),
            AgentError::Unavailable { .. }
        ));
    }

    /// Somebody who exported the variable wrongly should be told the same thing
    /// as somebody who did not export it at all.
    #[test]
    fn a_blank_credential_counts_as_no_credential() {
        assert!(
            without_a_key()
                .with_credential("   ")
                .availability()
                .is_err()
        );
    }

    /// The name goes into the audit record, so it has to be a name.
    #[test]
    fn the_provider_names_itself_without_naming_a_credential() {
        assert_eq!(without_a_key().name(), "openai");
    }

    /// The whole path from a request to a validated plan, with no credential
    /// and no network.
    #[test]
    fn a_plan_is_parsed_and_validated_without_touching_the_network() {
        let (transport, _) = Scripted::answering(replying_with(A_PLAN));
        let openai = provider(transport).with_credential("sk-test-not-a-real-key");

        let attempt = openai.plan("compare two CSV files").expect("a plan");

        assert_eq!(attempt.result.summary, "compares two CSV files");
        assert_eq!(attempt.usage.input_tokens, 11);
        assert_eq!(attempt.usage.output_tokens, 22);
    }

    /// The credential reaches the transport in a header, which is the only
    /// place anything in Ephemeral is allowed to put one — never the endpoint,
    /// never the body, both of which are recorded or shown.
    #[test]
    fn the_credential_travels_in_a_header_and_nowhere_else() {
        let (transport, seen) = Scripted::answering(replying_with(A_PLAN));
        let openai = provider(transport)
            .with_base_url("https://models.example.invalid/v1")
            .with_credential("sk-test-not-a-real-key");

        openai.plan("compare two CSV files").expect("a plan");

        let sent = seen.lock().expect("the log");
        let (endpoint, headers, body) = sent.first().expect("one request");

        assert_eq!(
            endpoint,
            "https://models.example.invalid/v1/chat/completions"
        );
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "authorization"
                    && value == "Bearer sk-test-not-a-real-key"),
            "{headers:?}"
        );
        assert!(
            !body.to_string().contains("sk-test-not-a-real-key"),
            "a credential in the body would be recorded with the request"
        );
    }

    /// A reply that does not parse is an error carrying what came back, not a
    /// best-effort guess.
    #[test]
    fn an_unreadable_reply_is_an_error_that_shows_what_arrived() {
        let (transport, _) = Scripted::answering(replying_with("this is not JSON at all"));
        let openai = provider(transport).with_credential("sk-test-not-a-real-key");

        let error = openai.plan("x").expect_err("nonsense is not a plan");

        assert!(
            matches!(&error, AgentError::Unreadable { raw, .. } if raw.contains("not JSON")),
            "{error:?}"
        );
    }

    /// A ceiling sent under a name the service does not read is no ceiling, so
    /// a value that names neither field is refused rather than defaulted past.
    #[test]
    fn a_ceiling_field_nobody_recognises_is_refused_rather_than_guessed() {
        let mut openai = provider(Box::new(Forbidden)).with_credential("sk-test-not-a-real-key");
        openai.ceiling = Err("maximum_tokens".to_owned());

        let error = openai.availability().expect_err("that is not a field");
        let message = error.to_string();

        assert!(message.contains(CEILING_VARIABLE), "{message}");
        assert!(message.contains("max_completion_tokens"), "{message}");

        // And nothing is sent: `Forbidden` panics if it is.
        assert!(openai.plan("x").is_err());
    }

    /// The dialect reaches the request, which is the only place it matters.
    #[test]
    fn the_older_dialect_is_what_goes_out_when_it_is_chosen() {
        let (transport, seen) = Scripted::answering(replying_with(A_PLAN));
        let openai = provider(transport)
            .with_ceiling(Ceiling::Legacy)
            .with_credential("sk-test-not-a-real-key");

        openai.plan("x").expect("a plan");

        let sent = seen.lock().expect("the log");
        let (_, _, body) = sent.first().expect("one request");

        assert!(body.get("max_tokens").is_some(), "{body}");
    }

    #[test]
    fn a_model_and_a_base_url_can_be_chosen() {
        let openai = without_a_key()
            .with_model("gpt-5-mini")
            .with_base_url("http://127.0.0.1:8000/v1/");

        assert_eq!(openai.model, "gpt-5-mini");
        assert_eq!(
            openai.endpoint(),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
    }
}
