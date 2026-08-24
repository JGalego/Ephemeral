//! How a confined application reaches the network, without a socket.
//!
//! There are no sockets in this WASI implementation and there will not be: an
//! interpreter that could open one on iOS would be an interpreter Apple's rules
//! have an opinion about, and a phone's TLS, certificate policy and background
//! behaviour belong to the platform ([ADR-0017]). So an application does not
//! connect to anything. It *describes a request*, and something outside the
//! sandbox decides whether to make it.
//!
//! That something is a [`Reach`], supplied by whoever is running the
//! application. On a desktop it spawns `curl`; on a handset it is the same host
//! callback that already carries a request to a model provider. **This crate
//! implements neither.** It has no HTTP client, opens no socket and resolves no
//! name, which is what keeps "the sandbox cannot reach the network" a fact
//! about the code rather than a promise about its behaviour.
//!
//! ## What is enforced here, and what is not
//!
//! The *policy* is enforced here, in [`super::engine`], against the grant the
//! person made — not by the host. A phone application deciding for itself which
//! destinations are allowed would be a second copy of the permission model, in
//! a different language, on a different release cycle. The host is asked to
//! perform a request that has already been checked.
//!
//! A `Reach` is therefore trusted to make the request it is given and nothing
//! more. It is not trusted to decide *whether* to.
//!
//! [ADR-0017]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0017-mobile-generates-through-a-host-transport.md

/// One request an application asked for.
///
/// Deliberately small. Two methods, one URL, one body, no headers: an
/// application that could set arbitrary headers on a request the host makes is
/// an application that can attach a credential it should never have seen, and
/// nothing a generated application legitimately does needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    /// `GET` or `POST`, and nothing else.
    pub method: Method,

    /// Where it is going, as the application wrote it.
    ///
    /// Already checked against the grant by the time a [`Reach`] sees it.
    pub url: String,

    /// What to send. Empty for a [`Method::Get`].
    pub body: String,
}

/// The two methods an application may ask for.
///
/// Not a string, so a host cannot be handed something it did not expect and no
/// application can smuggle a `CONNECT` past a comparison somebody wrote by
/// hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Reading something.
    Get,

    /// Sending something.
    Post,
}

impl Method {
    /// The word that goes on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }

    /// Reads a method an application named, or nothing if it named another.
    #[must_use]
    pub fn of(written: &str) -> Option<Self> {
        match written.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            _ => None,
        }
    }
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// The HTTP status, or zero where the host cannot report one.
    ///
    /// Zero is not a failure. Some hosts — the phone callback among them —
    /// return a body and no status, and reporting a `0` an application can see
    /// is more honest than inventing a `200` it might branch on.
    pub status: u16,

    /// The body, as text.
    pub body: String,
}

/// Something outside the sandbox that will make a request on an application's
/// behalf.
///
/// Implemented by callers, never by this crate.
pub trait Reach: Send + Sync {
    /// Makes one request, or says in a person's words why it could not.
    ///
    /// # Errors
    ///
    /// A sentence describing what went wrong, which reaches the application as
    /// the body of a refusal and reaches a person in the run's output.
    fn fetch(&self, request: &Outbound) -> Result<Answered, String>;
}

/// The most a single request or reply may carry, in bytes.
///
/// A bound rather than a preference. Without one, an application with a
/// network grant could ask the host to hold an arbitrary amount of memory on
/// its behalf — outside the store's own limit, which only bounds what the
/// module allocates for itself.
pub const MOST_ONE_BODY: usize = 1024 * 1024;

/// The most requests one run may make.
///
/// Fuel bounds the instructions a module executes, and a host call costs almost
/// none of it: the waiting happens outside the interpreter. So without this,
/// "it cannot run forever" would be true of the module and false of the run —
/// an application could sit in a loop making requests, having spent nearly no
/// fuel, for as long as the other end kept answering.
pub const MOST_REQUESTS: u32 = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_two_methods_are_understood() {
        assert_eq!(Method::of("get"), Some(Method::Get));
        assert_eq!(Method::of(" POST "), Some(Method::Post));

        for smuggled in ["CONNECT", "PUT", "DELETE", "TRACE", "", "GET POST"] {
            assert_eq!(
                Method::of(smuggled),
                None,
                "{smuggled} is not one of the two"
            );
        }
    }
}
