//! Whether a URL is somewhere on this machine.
//!
//! The local provider makes one promise — the intent does not leave the machine
//! — and this module is the whole of it. Everything else in the crate is the
//! OpenAI wire format with a different default; this is the part that would be
//! a lie if it were wrong.
//!
//! A pure function over a string, so the promise is a set of tests rather than
//! a claim. The interesting cases are the ones that *look* local:
//! `http://127.0.0.1@evil.example`, where the loopback address is a username;
//! `http://127.0.0.1.evil.example`, where it is a subdomain; and
//! `http://localhost.evil.example`, where it is a label. All three are refused.

/// Whether requests to `url` stay on this machine.
///
/// Conservative in both directions that matter. It accepts only `http` and
/// `https`, refuses any authority carrying userinfo — `user@host` is how a
/// loopback address is made to look like the destination when it is not — and
/// then asks the standard library whether what is left is a loopback address.
///
/// It also refuses some things that would in fact have been local: `127.1`,
/// which curl would expand, and a bare IPv6 address written without brackets.
/// Refusing a working configuration is a nuisance; accepting a remote one would
/// break the only promise this provider makes.
#[must_use]
pub fn is_on_this_machine(url: &str) -> bool {
    host_of(url).is_some_and(|host| is_loopback(&host))
}

/// The host part of a URL, if it has a shape this understands.
fn host_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;

    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|authority| !authority.is_empty())?;

    // `http://127.0.0.1@evil.example/` connects to evil.example. Nothing
    // Ephemeral sends needs userinfo, so an authority carrying any is refused
    // rather than parsed — there is no benign case to preserve.
    if authority.contains('@') {
        return None;
    }

    // A bracketed IPv6 literal, with or without a port.
    if let Some(rest) = authority.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if !after.is_empty() && !is_port(after) {
            return None;
        }
        return Some(inside.to_ascii_lowercase());
    }

    // Otherwise a name or an IPv4 address, with or without a port.
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if is_port(port) => host,
        Some(_) => return None,
        None => authority,
    };

    // An IPv6 address has to be bracketed in a URL, so a host that still has a
    // colon in it is not one this understands — `::1:8000` is an address and a
    // port to a reader and an address to a parser, and guessing which would be
    // guessing about where a request goes.
    if host.contains(':') || host.is_empty() {
        return None;
    }

    Some(host.to_ascii_lowercase())
}

/// Whether what follows a colon is a port and not part of an address.
fn is_port(text: &str) -> bool {
    let digits = text.strip_prefix(':').unwrap_or(text);

    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}

/// Whether a host names this machine.
///
/// `localhost` and anything under `.localhost` are reserved for exactly this by
/// [RFC 6761], and everything else has to be an address the standard library
/// agrees is a loopback one — which is `127.0.0.0/8` and `::1`, and is not
/// `0.0.0.0`, an address to bind rather than one to reach.
///
/// [RFC 6761]: https://www.rfc-editor.org/rfc/rfc6761#section-6.3
fn is_loopback(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }

    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configurations somebody actually runs a model under.
    #[test]
    fn the_ordinary_local_endpoints_are_local() {
        for url in [
            "http://127.0.0.1:11434/v1/chat/completions",
            "http://localhost:11434/v1/chat/completions",
            "http://LOCALHOST:8080/v1/chat/completions",
            "http://127.0.0.1/v1",
            "https://localhost:8443/v1/chat/completions",
            "http://[::1]:8000/v1/chat/completions",
            "http://[::1]/v1",
            "http://127.4.5.6:1234/v1",
            "http://ollama.localhost:11434/v1",
        ] {
            assert!(is_on_this_machine(url), "{url} is on this machine");
        }
    }

    /// The whole reason this function exists. Each of these looks local at a
    /// glance and sends the user's intent somewhere else.
    #[test]
    fn an_endpoint_that_only_looks_local_is_refused() {
        for url in [
            // The loopback address as userinfo: this connects to evil.example.
            "http://127.0.0.1@evil.example/v1/chat/completions",
            "http://localhost@evil.example/v1",
            "http://user:pass@127.0.0.1:11434/v1",
            // The loopback address as a label.
            "http://127.0.0.1.evil.example/v1",
            "http://localhost.evil.example/v1",
            "http://notlocalhost/v1",
            // The loopback address in the path, where it decides nothing.
            "http://evil.example/127.0.0.1/v1",
            "http://evil.example/?host=localhost",
            "http://evil.example/#localhost",
            // A machine on the network is not this machine.
            "http://192.168.1.10:11434/v1",
            "http://10.0.0.5:11434/v1",
            "http://0.0.0.0:11434/v1",
            "https://api.openai.com/v1/chat/completions",
        ] {
            assert!(!is_on_this_machine(url), "{url} is not on this machine");
        }
    }

    /// A URL this cannot read is not a local URL. There is no benefit of the
    /// doubt available here.
    #[test]
    fn anything_unparseable_is_refused() {
        for url in [
            "",
            "127.0.0.1:11434",
            "localhost",
            "ftp://127.0.0.1/v1",
            "file:///etc/passwd",
            "http://",
            "http:///v1",
            "http://[::1/v1",
            "http://::1:8000/v1",
            "http://127.0.0.1:notaport/v1",
        ] {
            assert!(!is_on_this_machine(url), "{url} should not be accepted");
        }
    }
}
