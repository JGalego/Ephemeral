//! # Anthropic provider
//!
//! A real [`AgentProvider`], so Ephemeral can build something somebody actually
//! asked for rather than a fixture.
//!
//! It lives in its own crate for one reason: `ephemeral-agent` is guarded by CI
//! against containing any network client, and that guard is what makes "CI never
//! makes a live model call" a fact rather than a policy ([ADR-0016]). Adding a
//! transport there would have relaxed the first mechanical check that got in the
//! way, which is how such checks stop meaning anything.
//!
//! ## What is tested and what is not
//!
//! Prompt construction, request bodies, response parsing, capability
//! translation and error mapping are pure functions in [`wire`], and all of
//! them are tested. [`transport`] hands a string to `curl` and is the only part
//! CI cannot exercise, because doing so would need a credential.
//!
//! That split is the point: the untested surface is roughly thirty lines with
//! no decisions in it.
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

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod transport;
pub mod wire;

use ephemeral_agent::dialogue;
use ephemeral_agent::{
    AgentError, AgentProvider, Attempt,
    plan::{GeneratedApp, Plan, RepairAttempt, SourceFile},
};

/// What this provider is called, in the interface and the audit record.
pub const NAME: &str = "anthropic";

/// The environment variable the credential is read from.
pub const API_KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";

/// Generates applications with a hosted model.
pub struct AnthropicProvider {
    api_key: Option<String>,
    model: String,
    endpoint: String,
    transport: Box<dyn transport::Transport>,
}

#[cfg(feature = "curl")]
impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The credential, from the environment, if there is one there.
///
/// A desktop convention. A phone has no environment to read, which is why the
/// credential can also be supplied directly.
#[must_use]
fn credential_from_environment() -> Option<String> {
    std::env::var(API_KEY_VARIABLE)
        .ok()
        .filter(|key| !key.trim().is_empty())
}

impl AnthropicProvider {
    /// A provider reading its credential from the environment, sending through
    /// `curl`.
    ///
    /// Read once, at construction, so it is not fetched again per request — and
    /// held as an `Option` so that "no credential" is a state this type can
    /// describe rather than something discovered mid-generation.
    #[cfg(feature = "curl")]
    #[must_use]
    pub fn new() -> Self {
        Self::with_transport(Box::new(transport::Curl))
    }

    /// The same, sending through a transport the caller supplies.
    ///
    /// This is the constructor a phone uses. Spawning `curl` is impossible on
    /// iOS, and reading the credential from an environment variable is not how
    /// a mobile application holds a secret either — both are desktop answers,
    /// and both are supplied from outside rather than assumed here.
    #[must_use]
    pub fn with_transport(transport: Box<dyn transport::Transport>) -> Self {
        Self {
            api_key: credential_from_environment(),
            model: wire::DEFAULT_MODEL.to_owned(),
            endpoint: wire::ENDPOINT.to_owned(),
            transport,
        }
    }

    /// The same, with a credential the caller already holds.
    ///
    /// For a platform whose secret store is not the environment — a Keychain,
    /// a Keystore, a Credential Manager.
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

    /// The credential, or the reason there is not one.
    fn key(&self) -> Result<&str, AgentError> {
        self.api_key
            .as_deref()
            .ok_or_else(|| AgentError::Unavailable {
                provider: NAME.to_owned(),
                reason: format!(
                    "no API key. Set {API_KEY_VARIABLE}, or use `--provider mock` to build the \
                 example application without one."
                ),
            })
    }

    /// One round trip: send, read the text, read the JSON in it.
    fn ask(
        &self,
        request: &serde_json::Value,
    ) -> Result<(serde_json::Value, ephemeral_agent::Usage), AgentError> {
        let response = self.transport.send(&self.endpoint, self.key()?, request)?;
        let usage = wire::usage_from(&response);
        let text = wire::text_from(&response)?;

        Ok((dialogue::json_from(NAME, &text)?, usage))
    }
}

impl AgentProvider for AnthropicProvider {
    fn name(&self) -> &'static str {
        NAME
    }

    fn availability(&self) -> Result<(), AgentError> {
        // Deliberately does not reach the network: an unavailable provider is a
        // diagnosis `ephemeral doctor` reports, not something discovered by
        // trying.
        self.key().map(|_| ())
    }

    fn plan(&self, intent: &str) -> Result<Attempt<Plan>, AgentError> {
        let (value, usage) = self.ask(&wire::plan_request(&self.model, intent))?;

        Ok(Attempt::new(dialogue::plan_from(NAME, &value)?, usage))
    }

    fn generate(&self, plan: &Plan) -> Result<Attempt<GeneratedApp>, AgentError> {
        let (value, usage) = self.ask(&wire::generate_request(&self.model, plan))?;

        Ok(Attempt::new(dialogue::app_from(NAME, &value, plan)?, usage))
    }

    fn repair(
        &self,
        _app: &GeneratedApp,
        files: &[SourceFile],
        failure: &str,
    ) -> Result<Attempt<RepairAttempt>, AgentError> {
        let (value, usage) = self.ask(&wire::repair_request(&self.model, files, failure))?;

        Ok(Attempt::new(dialogue::repair_from(NAME, &value)?, usage))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport that answers from a script instead of the network.
    ///
    /// This is what the trait bought: the provider can now be driven
    /// end-to-end in CI, with no credential, no `curl`, and no network. Before
    /// it, everything from `plan()` down to response parsing was reachable only
    /// by making a real call.
    struct Scripted(serde_json::Value);

    impl transport::Transport for Scripted {
        fn send(
            &self,
            _endpoint: &str,
            _api_key: &str,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, AgentError> {
            Ok(self.0.clone())
        }
    }

    /// A transport that must never be reached.
    struct Forbidden;

    impl transport::Transport for Forbidden {
        fn send(
            &self,
            _endpoint: &str,
            _api_key: &str,
            _request: &serde_json::Value,
        ) -> Result<serde_json::Value, AgentError> {
            panic!("the transport was used without a credential");
        }
    }

    /// A reply carrying `text` the way the API frames it.
    fn replying_with(text: &str) -> serde_json::Value {
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "usage": { "input_tokens": 11, "output_tokens": 22 },
        })
    }

    fn without_a_key() -> AnthropicProvider {
        AnthropicProvider {
            api_key: None,
            model: wire::DEFAULT_MODEL.to_owned(),
            endpoint: wire::ENDPOINT.to_owned(),
            transport: Box::new(Forbidden),
        }
    }

    fn answering(reply: serde_json::Value) -> AnthropicProvider {
        AnthropicProvider::with_transport(Box::new(Scripted(reply)))
            .with_credential("sk-test-not-a-real-key")
    }

    /// A missing credential is a diagnosis with a remedy, not a failure
    /// somebody discovers halfway through a generation run.
    #[test]
    fn a_missing_credential_is_reported_before_anything_is_attempted() {
        let provider = without_a_key();

        let error = provider.availability().expect_err("no key, no provider");
        let message = error.to_string();

        assert!(message.contains(API_KEY_VARIABLE), "{message}");
        assert!(message.contains("mock"), "it should offer the way forward");
    }

    /// Nothing is sent without a credential, so a missing one cannot become a
    /// mysterious network error.
    #[test]
    fn nothing_is_attempted_without_a_credential() {
        let provider = without_a_key();

        assert!(matches!(
            provider.plan("x").unwrap_err(),
            AgentError::Unavailable { .. }
        ));
    }

    /// An empty or blank variable is the same as an absent one. Somebody who
    /// exported it wrongly should be told the same thing as somebody who did
    /// not export it at all.
    #[test]
    fn a_blank_credential_counts_as_no_credential() {
        // SAFETY-adjacent: this mutates process state, so the assertion is made
        // through the same path the constructor uses rather than by inspection.
        let blank = AnthropicProvider::with_transport(Box::new(Forbidden)).with_credential("   ");

        assert!(blank.availability().is_err());
    }

    /// The name goes into the audit record, so it has to be a name.
    #[test]
    fn the_provider_names_itself_without_naming_a_credential() {
        assert_eq!(without_a_key().name(), "anthropic");
    }

    /// The whole path from a request to a validated plan, with no credential
    /// and no network. This was unreachable before the transport was a seam.
    #[test]
    fn a_plan_is_parsed_and_validated_without_touching_the_network() {
        let provider = answering(replying_with(
            r#"{"name":"CSV Comparator","summary":"compares two CSV files",
                "runtime":"docker","image":"python:3.12-slim",
                "interface":"command_line","permissions":[]}"#,
        ));

        let attempt = provider.plan("compare two CSV files").expect("a plan");

        assert_eq!(attempt.result.summary, "compares two CSV files");
        assert_eq!(attempt.usage.input_tokens, 11);
        assert_eq!(attempt.usage.output_tokens, 22);
    }

    /// A reply that does not parse is an error carrying what came back, not a
    /// best-effort guess. Also reachable only through a fake transport.
    #[test]
    fn an_unreadable_reply_is_an_error_that_shows_what_arrived() {
        let provider = answering(replying_with("this is not JSON at all"));

        let error = provider.plan("x").expect_err("nonsense is not a plan");

        assert!(
            matches!(&error, AgentError::Unreadable { raw, .. } if raw.contains("not JSON")),
            "{error:?}"
        );
    }

    #[test]
    fn a_model_can_be_chosen() {
        let provider = without_a_key().with_model("claude-opus-5");
        assert_eq!(provider.model, "claude-opus-5");
    }
}
