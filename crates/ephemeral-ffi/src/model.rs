//! Which model a phone talks to.
//!
//! This crate used to build an Anthropic provider and nothing else, with a
//! comment explaining that wiring a second one meant passing a whole header set
//! across a published ABI and was therefore "a decision rather than a detail".
//! That was true and it was still the wrong call: it made the platform decide
//! the vendor. A desktop user picks a provider per run and points it at Groq,
//! Together, a company gateway or a model on the next desk; a phone user got
//! Anthropic or nothing, and no amount of configuration anywhere could change
//! it. Nothing about a phone justifies that — generating is one HTTPS request,
//! and which company answers it is a person's business.
//!
//! So the header set does cross the boundary now, and the host chooses the
//! provider like any other client. What is here is the choice, how it is
//! written down, and the catalogue a host reads to build a picker without
//! hardcoding a list that will go stale.
//!
//! The credential is deliberately not part of the choice. It comes from the
//! platform's secure store through [`crate::ephemeral_set_credential`], and
//! keeping it out of this structure is what lets the choice be saved in
//! ordinary preferences, echoed back to the host, and written in a log.

use ephemeral_agent::AgentProvider;
use ephemeral_agent::transport::Transport;
use serde::{Deserialize, Serialize};

/// The provider used when a host never chooses one.
///
/// Anthropic, because that is what this ABI did before it could be told
/// otherwise, and a host that has shipped should not change behaviour on an
/// upgrade it did not ask for.
pub const DEFAULT_PROVIDER: &str = ephemeral_provider_anthropic::NAME;

/// What a host chose, as it travels across the boundary.
///
/// Every field but the name is optional and every absent one means "whatever
/// that provider's default is". A host that knows only the name can send only
/// the name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    /// Which provider: `mock`, `anthropic`, or `openai`.
    pub provider: String,

    /// The service's base URL, for anything that is not the vendor's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Which model, by whatever name the service knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Which field bounds the reply, for the OpenAI format only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ceiling: Option<String>,
}

impl Default for Choice {
    fn default() -> Self {
        Self {
            provider: DEFAULT_PROVIDER.to_owned(),
            base_url: None,
            model: None,
            ceiling: None,
        }
    }
}

impl Choice {
    /// Reads a choice from what the host sent.
    ///
    /// # Errors
    ///
    /// If the text is not JSON, or names a provider this build does not have.
    /// A misspelled provider is refused rather than defaulted past: silently
    /// generating with something other than what somebody chose is worse than
    /// not generating.
    pub fn parse(json: &str) -> Result<Self, String> {
        let chosen: Self = serde_json::from_str(json)
            .map_err(|error| format!("that is not a provider configuration: {error}"))?;

        if !catalogue().iter().any(|one| one.name == chosen.provider) {
            let known: Vec<&str> = catalogue().iter().map(|one| one.name).collect();
            return Err(format!(
                "there is no provider called {}. This build has: {}.",
                chosen.provider,
                known.join(", ")
            ));
        }

        Ok(chosen)
    }

    /// Builds the provider this choice describes.
    ///
    /// # Errors
    ///
    /// If the provider does not exist, or a setting it was given is one it
    /// cannot use.
    pub fn build(
        &self,
        credential: Option<&str>,
        transport: Box<dyn Transport>,
    ) -> Result<Box<dyn AgentProvider>, String> {
        match self.provider.as_str() {
            ephemeral_agent::mock::NAME => Ok(Box::new(ephemeral_agent::MockProvider::new())),

            ephemeral_provider_anthropic::NAME => {
                // A handset writes what a handset can run. This is the whole
                // difference between an application that works where it was
                // written and one that has to be carried to a computer.
                let mut provider =
                    ephemeral_provider_anthropic::AnthropicProvider::with_transport(transport)
                        .writing(ephemeral_agent::dialogue::Target::Script);
                if let Some(base) = &self.base_url {
                    provider = provider.with_base_url(base);
                }
                if let Some(model) = &self.model {
                    provider = provider.with_model(model.clone());
                }
                if let Some(key) = credential {
                    provider = provider.with_credential(key);
                }
                Ok(Box::new(provider))
            }

            ephemeral_provider_openai::NAME => {
                let mut provider =
                    ephemeral_provider_openai::OpenAiProvider::with_transport(transport)
                        .writing(ephemeral_agent::dialogue::Target::Script);
                if let Some(base) = &self.base_url {
                    provider = provider.with_base_url(base);
                }
                if let Some(model) = &self.model {
                    provider = provider.with_model(model.clone());
                }
                if let Some(field) = &self.ceiling {
                    provider = provider.with_ceiling(ceiling(field)?);
                }
                if let Some(key) = credential {
                    provider = provider.with_credential(key);
                }
                Ok(Box::new(provider))
            }

            other => Err(format!("there is no provider called {other}")),
        }
    }
}

/// Which field a service reads the reply ceiling from.
///
/// Refused rather than defaulted, for the reason the OpenAI provider gives: a
/// bound sent under a name the service does not read is no bound at all, and an
/// unbounded reply is an unbounded bill on somebody's phone.
fn ceiling(field: &str) -> Result<ephemeral_provider_openai::wire::Ceiling, String> {
    match field {
        "max_completion_tokens" => Ok(ephemeral_provider_openai::wire::Ceiling::Current),
        "max_tokens" => Ok(ephemeral_provider_openai::wire::Ceiling::Legacy),
        other => Err(format!(
            "{other} is not a field a reply ceiling can be sent in. It is \
             `max_completion_tokens`, or `max_tokens` for a service that copied this format \
             before OpenAI renamed it."
        )),
    }
}

/// One provider, as a host describes it to a person.
///
/// Sent across the boundary so that a picker on a phone is built from what this
/// build actually has, rather than from a list written into an application that
/// ships on its own schedule and goes stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Described {
    /// The name to send back in a [`Choice`].
    pub name: &'static str,

    /// One line, for a person choosing.
    pub what: &'static str,

    /// Whether it needs a credential before it can do anything.
    pub needs_credential: bool,

    /// Which fields of a [`Choice`] this provider reads. A host should show
    /// these and hide the rest, rather than offering a model box for the mock.
    pub configurable: &'static [&'static str],

    /// What it uses when a host sets nothing, so a field can be pre-filled
    /// rather than left blank and mysterious.
    pub base_url: Option<&'static str>,

    /// The same, for the model.
    pub model: Option<&'static str>,
}

/// Every provider a phone can be pointed at, in the order a person meets them.
///
/// The mock first, because it is the one that needs no account anywhere and is
/// how somebody sees the whole flow before deciding whether to hand a phone a
/// credential. `local` is deliberately absent: it exists to keep an intent on
/// the machine that generated it, and it refuses any endpoint that is not
/// loopback — which on a phone means a model server running on that phone.
/// Anything else somebody means by "local" is another machine, and that is
/// `openai` with a base URL, which is what it is.
#[must_use]
pub fn catalogue() -> Vec<Described> {
    vec![
        Described {
            name: ephemeral_agent::mock::NAME,
            what: "A fixed example application. No credential, no network, no bill.",
            needs_credential: false,
            configurable: &[],
            base_url: None,
            model: None,
        },
        Described {
            name: ephemeral_provider_anthropic::NAME,
            what: "Anthropic, or a gateway that speaks its API.",
            needs_credential: true,
            configurable: &["base_url", "model"],
            base_url: Some(ephemeral_provider_anthropic::wire::BASE_URL),
            model: Some(ephemeral_provider_anthropic::wire::DEFAULT_MODEL),
        },
        Described {
            name: ephemeral_provider_openai::NAME,
            what: "OpenAI, Groq, Together, OpenRouter, a company gateway — anything \
                   that speaks the chat completions format.",
            needs_credential: true,
            configurable: &["base_url", "model", "ceiling"],
            base_url: Some(ephemeral_provider_openai::wire::BASE_URL),
            model: Some(ephemeral_provider_openai::wire::DEFAULT_MODEL),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Nowhere;

    impl Transport for Nowhere {
        fn send(
            &self,
            _request: &ephemeral_agent::transport::HttpRequest<'_>,
        ) -> Result<serde_json::Value, ephemeral_agent::AgentError> {
            unimplemented!("no test here sends anything")
        }
    }

    fn built(json: &str) -> Result<Box<dyn AgentProvider>, String> {
        Choice::parse(json)?.build(Some("a-key"), Box::new(Nowhere))
    }

    /// The name of what was built, or why it could not be. A provider is not
    /// `Debug`, and a test that only needs to know which one it got should not
    /// require it to be.
    fn name_of(json: &str) -> Result<&'static str, String> {
        built(json).map(|provider| provider.name())
    }

    /// The whole point of the change: a phone can be pointed at something that
    /// is not Anthropic.
    #[test]
    fn a_phone_can_be_pointed_at_anything_that_speaks_a_known_format() {
        let groq = built(
            r#"{"provider":"openai","base_url":"https://api.groq.com/openai/v1",
                "model":"llama-3.3-70b-versatile"}"#,
        )
        .expect("a provider");

        assert_eq!(groq.name(), "openai");
        assert!(
            groq.availability().is_ok(),
            "with a credential it is ready to use"
        );
    }

    /// A name is enough. Everything else has a default and a host that knows
    /// only "openai" must not have to invent a base URL.
    #[test]
    fn a_name_on_its_own_is_a_complete_choice() {
        for name in ["mock", "anthropic", "openai"] {
            let json = format!(r#"{{"provider":"{name}"}}"#);
            assert_eq!(
                built(&json).expect("a provider").name(),
                name,
                "{name} should be choosable by name alone"
            );
        }
    }

    /// The default has to stay what it was. A host that shipped against this
    /// ABI and never chose a provider must not silently start using a
    /// different company on an upgrade.
    #[test]
    fn choosing_nothing_is_still_anthropic() {
        assert_eq!(Choice::default().provider, "anthropic");
    }

    /// A misspelling is refused, and the refusal says what does exist. Falling
    /// back to a default here would generate with a company somebody did not
    /// pick.
    #[test]
    fn a_provider_that_does_not_exist_is_refused_by_name() {
        let error = Choice::parse(r#"{"provider":"gorq"}"#).expect_err("no such provider");

        assert!(error.contains("gorq"), "it should quote what was asked for");
        assert!(error.contains("openai"), "and say what there is: {error}");
    }

    /// A ceiling field the service does not read is no ceiling. Refused rather
    /// than quietly replaced with the default.
    #[test]
    fn a_ceiling_nobody_reads_is_refused() {
        let error = name_of(r#"{"provider":"openai","ceiling":"max_output_tokens"}"#)
            .expect_err("that is not a field");

        assert!(error.contains("max_completion_tokens"), "{error}");
    }

    /// A choice survives the round trip a host will actually do: save it,
    /// read it back, send it in.
    #[test]
    fn a_choice_written_down_can_be_read_back() {
        let chosen = Choice {
            provider: "openai".to_owned(),
            base_url: Some("https://api.groq.com/openai/v1".to_owned()),
            model: Some("llama-3.3-70b-versatile".to_owned()),
            ceiling: Some("max_tokens".to_owned()),
        };

        let written = serde_json::to_string(&chosen).expect("it serialises");
        assert_eq!(Choice::parse(&written).expect("it parses back"), chosen);
    }

    /// The catalogue is what a host builds a picker from, so everything in it
    /// has to be a thing that can actually be chosen.
    #[test]
    fn everything_in_the_catalogue_can_be_chosen() {
        for described in catalogue() {
            let json = format!(r#"{{"provider":"{}"}}"#, described.name);
            let provider =
                built(&json).unwrap_or_else(|error| panic!("{}: {error}", described.name));

            assert_eq!(provider.name(), described.name);
            assert!(
                !described.what.is_empty(),
                "{} says nothing",
                described.name
            );

            if described.needs_credential {
                assert!(
                    described.base_url.is_some() && described.model.is_some(),
                    "{} is configurable, so a host needs its defaults to show",
                    described.name
                );
            }
        }
    }

    /// The mock needs no credential, and that has to be true rather than
    /// merely claimed in the catalogue — it is the whole reason somebody can
    /// see the flow work before handing a phone a key.
    #[test]
    fn the_mock_works_with_no_credential_at_all() {
        let provider = Choice::parse(r#"{"provider":"mock"}"#)
            .expect("a choice")
            .build(None, Box::new(Nowhere))
            .expect("a provider");

        assert!(provider.availability().is_ok());
    }
}
