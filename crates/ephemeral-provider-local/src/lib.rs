//! # Local provider
//!
//! Generation with a model on this machine, and the only honest answer to "my
//! intent leaves the machine" ([ADR-0019]).
//!
//! Every other provider sends what a person asked for to somebody else's
//! computer. That is stated plainly in the [threat model] and it is not
//! something a promise can fix — a hosted provider learns the intent, and the
//! mitigation available is choice rather than prevention. This is the choice.
//!
//! ## What makes it local
//!
//! Not the name, and not the default. [`endpoint::is_on_this_machine`] decides,
//! and it is checked before every request: an endpoint that is not loopback is
//! refused with an error naming `--provider openai`, which is the honest way to
//! reach a model server on another machine. The default endpoint is Ollama's,
//! because that is what most people have running.
//!
//! Reaching it needs no credential, and none is sent. A server that insists on
//! one — vLLM started with `--api-key`, say — is configured through
//! [`API_KEY_VARIABLE`], which is deliberately *not* `OPENAI_API_KEY`: a hosted
//! credential must never be handed to a process on the machine merely because
//! both happen to speak the same protocol.
//!
//! ## Everything else is the OpenAI wire format
//!
//! Ollama, llama.cpp's server, LM Studio and vLLM all accept OpenAI's chat
//! completions API, so this crate borrows
//! [`ephemeral_provider_openai::wire`] rather than owning a second copy of it.
//! What it does not borrow is the response ceiling's field name: the servers
//! that copied the format copied it before OpenAI renamed `max_tokens`, so this
//! sends the older name.
//!
//! ## Nothing here is trusted
//!
//! A local model's output is validated exactly as a hosted one's is. A model
//! running on the user's own machine is not more trustworthy than a hosted one
//! — it is only more private, and privacy is not integrity.
//!
//! [ADR-0019]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0019-openai-compatible-and-a-local-model.md
//! [threat model]: https://github.com/JGalego/Ephemeral/blob/main/docs/security/threat-model.md

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod endpoint;

use ephemeral_agent::dialogue;
use ephemeral_agent::{
    AgentError, AgentProvider, Attempt,
    plan::{GeneratedApp, Plan, RepairAttempt, SourceFile},
    transport::{HttpRequest, Transport},
};
use ephemeral_provider_openai::wire::{self, Ceiling};

/// What this provider is called, in the interface and the audit record.
pub const NAME: &str = "local";

/// The base URL used unless one is named: Ollama's, on the loopback interface.
pub const BASE_URL: &str = "http://127.0.0.1:11434/v1";

/// The model used unless one is named.
///
/// A coding model small enough to run on an ordinary laptop. It is a starting
/// point rather than a recommendation — generation asks a model to produce a
/// whole application as one valid JSON object, and a small model fails that
/// often enough to be worth saying out loud.
pub const DEFAULT_MODEL: &str = "qwen2.5-coder";

/// The environment variable that points this at a different local server.
pub const BASE_URL_VARIABLE: &str = "EPHEMERAL_LOCAL_BASE_URL";

/// The environment variable that names the model.
pub const MODEL_VARIABLE: &str = "EPHEMERAL_LOCAL_MODEL";

/// The environment variable for a local server that insists on a credential.
///
/// Deliberately not `OPENAI_API_KEY`. A hosted credential lying around in the
/// environment must not be sent to a process on this machine just because it
/// speaks the same protocol.
pub const API_KEY_VARIABLE: &str = "EPHEMERAL_LOCAL_API_KEY";

/// Generates applications with a model server on this machine.
pub struct LocalProvider {
    api_key: Option<String>,
    model: String,
    endpoint: String,
    transport: Box<dyn Transport>,
}

#[cfg(feature = "curl")]
impl Default for LocalProvider {
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

impl LocalProvider {
    /// A provider configured from the environment, sending through `curl`.
    #[cfg(feature = "curl")]
    #[must_use]
    pub fn new() -> Self {
        Self::with_transport(Box::new(ephemeral_agent::transport::Curl::for_provider(
            NAME,
        )))
    }

    /// The same, sending through a transport the caller supplies.
    #[must_use]
    pub fn with_transport(transport: Box<dyn Transport>) -> Self {
        let base = from_environment(BASE_URL_VARIABLE).unwrap_or_else(|| BASE_URL.to_owned());

        Self {
            api_key: from_environment(API_KEY_VARIABLE),
            model: from_environment(MODEL_VARIABLE).unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            // Kept as configured, even when it is not local. Quietly falling
            // back to the default would send the intent somewhere the person
            // did not choose, which is the one mistake this provider exists to
            // make impossible; it is refused instead, by name.
            endpoint: wire::endpoint_from(&base),
            transport,
        }
    }

    /// The same, with a particular model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The same, pointed at a local server whose base URL is `base`.
    #[must_use]
    pub fn with_base_url(mut self, base: &str) -> Self {
        self.endpoint = wire::endpoint_from(base);
        self
    }

    /// The same, with a credential for a local server that demands one.
    #[must_use]
    pub fn with_credential(mut self, api_key: impl Into<String>) -> Self {
        let api_key = api_key.into();
        self.api_key = Some(api_key).filter(|key| !key.trim().is_empty());
        self
    }

    /// Where this provider sends its requests.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Checks the endpoint before anything is sent to it.
    fn stays_on_this_machine(&self) -> Result<(), AgentError> {
        if endpoint::is_on_this_machine(&self.endpoint) {
            return Ok(());
        }

        Err(AgentError::Unavailable {
            provider: NAME.to_owned(),
            reason: format!(
                "{} is not on this machine, and `local` is the provider that promises the \
                 intent does not leave it. Point {BASE_URL_VARIABLE} at a loopback address, \
                 or use `--provider openai` with OPENAI_BASE_URL, which makes no such promise.",
                self.endpoint
            ),
        })
    }

    /// One round trip: send, read the text, read the JSON in it.
    fn ask(
        &self,
        request: &serde_json::Value,
    ) -> Result<(serde_json::Value, ephemeral_agent::Usage), AgentError> {
        // Checked here rather than only in `availability`, because a provider
        // built directly — by a test, by an embedder, by the FFI — never passes
        // through it. The guarantee is on the request, not on the diagnosis.
        self.stays_on_this_machine()?;

        let response = self.transport.send(&HttpRequest {
            endpoint: &self.endpoint,
            headers: wire::headers(self.api_key.as_deref()),
            body: request,
        })?;
        let usage = wire::usage_from(&response);
        let text = wire::text_from(NAME, &response)?;

        Ok((dialogue::json_from(NAME, &text)?, usage))
    }
}

impl AgentProvider for LocalProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn availability(&self) -> Result<(), AgentError> {
        // Deliberately does not reach the server: an unavailable provider is a
        // diagnosis with a remedy, not something discovered by trying. Whether
        // the model is loaded is the server's answer to give, and it gives it
        // clearly when the first request arrives.
        self.stays_on_this_machine()
    }

    fn plan(&self, intent: &str) -> Result<Attempt<Plan>, AgentError> {
        let (value, usage) = self.ask(&wire::plan_request(&self.model, Ceiling::Legacy, intent))?;

        Ok(Attempt::new(dialogue::plan_from(NAME, &value)?, usage))
    }

    fn generate(&self, plan: &Plan) -> Result<Attempt<GeneratedApp>, AgentError> {
        let (value, usage) =
            self.ask(&wire::generate_request(&self.model, Ceiling::Legacy, plan))?;

        Ok(Attempt::new(dialogue::app_from(NAME, &value, plan)?, usage))
    }

    fn repair(
        &self,
        _app: &GeneratedApp,
        files: &[SourceFile],
        failure: &str,
    ) -> Result<Attempt<RepairAttempt>, AgentError> {
        let (value, usage) = self.ask(&wire::repair_request(
            &self.model,
            Ceiling::Legacy,
            files,
            failure,
        ))?;

        Ok(Attempt::new(dialogue::repair_from(NAME, &value)?, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one request looked like by the time it reached the transport.
    type Sent = (String, Vec<(String, String)>, serde_json::Value);

    /// A transport that answers from a script and records what it was given.
    struct Scripted {
        reply: serde_json::Value,
        seen: std::sync::Arc<std::sync::Mutex<Vec<Sent>>>,
    }

    impl Scripted {
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
        fn send(&self, request: &HttpRequest<'_>) -> Result<serde_json::Value, AgentError> {
            panic!("a request was sent to {}", request.endpoint);
        }
    }

    /// A reply carrying `text` the way an OpenAI-compatible server frames it.
    fn replying_with(text: &str) -> serde_json::Value {
        serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": text },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 900, "completion_tokens": 120 },
        })
    }

    const A_PLAN: &str = r#"{"summary":"compares two CSV files",
        "runtime":"docker","image":"python:3.12-slim",
        "interface":"command_line","requests":[]}"#;

    /// Built by hand rather than from the environment, because a test that
    /// reads the environment passes or fails depending on the machine it runs
    /// on.
    fn provider(transport: Box<dyn Transport>) -> LocalProvider {
        LocalProvider {
            api_key: None,
            model: DEFAULT_MODEL.to_owned(),
            endpoint: wire::endpoint_from(BASE_URL),
            transport,
        }
    }

    /// The default is the promise's default, and a default that reached off the
    /// machine would break it before anybody configured anything.
    #[test]
    fn the_default_endpoint_is_on_this_machine() {
        assert!(endpoint::is_on_this_machine(&wire::endpoint_from(BASE_URL)));
        assert!(provider(Box::new(Forbidden)).availability().is_ok());
    }

    /// The refusal that makes `local` mean something. Nothing is sent, and the
    /// error names the provider that would have been the right one.
    #[test]
    fn an_endpoint_off_this_machine_is_refused_before_anything_is_sent() {
        let local = provider(Box::new(Forbidden)).with_base_url("https://api.openai.com/v1");

        let error = local.availability().expect_err("that is not this machine");
        let message = error.to_string();

        assert!(message.contains("not on this machine"), "{message}");
        assert!(message.contains(BASE_URL_VARIABLE), "{message}");
        assert!(
            message.contains("--provider openai"),
            "a refusal with no way forward is a dead end: {message}"
        );

        // And the `Forbidden` transport panicking is the assertion that nothing
        // was sent — including here, where `plan` is called directly rather
        // than through `availability`.
        assert!(matches!(
            local.plan("compare two CSV files").unwrap_err(),
            AgentError::Unavailable { .. }
        ));
    }

    /// The subtle one: a URL that reads as loopback and resolves elsewhere.
    /// Covered exhaustively in `endpoint`; asserted here because it is the
    /// provider, not the parser, that has to act on it.
    #[test]
    fn an_endpoint_that_only_looks_local_is_refused_too() {
        let local = provider(Box::new(Forbidden)).with_base_url("http://127.0.0.1@evil.example/v1");

        assert!(local.availability().is_err());
    }

    /// No credential is configured, so none is sent — not even one sitting in
    /// the environment for a hosted provider.
    #[test]
    fn no_credential_is_sent_to_a_local_server_by_default() {
        let (transport, seen) = Scripted::answering(replying_with(A_PLAN));
        let local = provider(transport);

        local.plan("compare two CSV files").expect("a plan");

        let sent = seen.lock().expect("the log");
        let (endpoint, headers, _) = sent.first().expect("one request");

        assert_eq!(endpoint, "http://127.0.0.1:11434/v1/chat/completions");
        assert!(
            headers.iter().all(|(name, _)| name != "authorization"),
            "{headers:?}"
        );
    }

    /// A local server started with a key of its own gets one, from its own
    /// variable.
    #[test]
    fn a_local_server_that_demands_a_credential_gets_the_local_one() {
        let (transport, seen) = Scripted::answering(replying_with(A_PLAN));
        let local = provider(transport).with_credential("local-server-token");

        local.plan("x").expect("a plan");

        let sent = seen.lock().expect("the log");
        let (_, headers, _) = sent.first().expect("one request");

        assert!(
            headers.iter().any(
                |(name, value)| name == "authorization" && value.contains("local-server-token")
            ),
            "{headers:?}"
        );
    }

    /// The whole path from a request to a validated plan, with no network and
    /// no model.
    #[test]
    fn a_plan_is_parsed_and_validated_without_touching_anything() {
        let (transport, _) = Scripted::answering(replying_with(A_PLAN));

        let attempt = provider(transport)
            .plan("compare two CSV files")
            .expect("a plan");

        assert_eq!(attempt.result.summary, "compares two CSV files");
        assert_eq!(attempt.usage.input_tokens, 900);
        assert_eq!(attempt.usage.output_tokens, 120);
    }

    /// A local model costs nothing, which is why a manifest's spend ceiling is
    /// optional — and why nothing here invents a price to fill it.
    #[test]
    fn generating_locally_costs_nothing() {
        let (transport, _) = Scripted::answering(replying_with(A_PLAN));

        let attempt = provider(transport).plan("x").expect("a plan");

        assert_eq!(attempt.usage.cents, 0);
        assert!(attempt.usage.describe().contains("at no cost"));
    }

    /// A local model is not a trusted model. Its output is read exactly as
    /// suspiciously as a hosted one's.
    #[test]
    fn an_unreadable_reply_from_a_local_model_is_still_an_error() {
        let (transport, _) = Scripted::answering(replying_with("sure! here is your app:"));

        let error = provider(transport)
            .plan("x")
            .expect_err("prose is not a plan");

        assert!(
            matches!(&error, AgentError::Unreadable { provider, .. } if provider == NAME),
            "{error:?}"
        );
    }

    /// The servers that copied this format copied it before the field was
    /// renamed, so an unbounded reply is what a modern name would get here.
    #[test]
    fn the_response_ceiling_uses_the_name_these_servers_know() {
        let (transport, seen) = Scripted::answering(replying_with(A_PLAN));

        provider(transport).plan("x").expect("a plan");

        let sent = seen.lock().expect("the log");
        let (_, _, body) = sent.first().expect("one request");

        assert!(body.get("max_tokens").is_some(), "{body}");
        assert!(body.get("max_completion_tokens").is_none(), "{body}");
    }

    /// The name goes into the audit record.
    #[test]
    fn the_provider_names_itself() {
        assert_eq!(provider(Box::new(Forbidden)).name(), "local");
    }

    #[test]
    fn a_model_can_be_chosen() {
        let local = provider(Box::new(Forbidden)).with_model("qwen3-coder");

        assert_eq!(local.model, "qwen3-coder");
    }
}
