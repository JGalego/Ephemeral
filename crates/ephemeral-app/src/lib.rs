//! What a generated application links against.
//!
//! An application confined by Ephemeral's WebAssembly runtime has no sockets
//! and never will ([ADR-0021]). What it has, once a person has allowed a
//! destination, is a pair of host functions it can *describe* a request to
//! ([ADR-0023]). Those are a C ABI: a raw pointer, a length, and a JSON
//! document assembled by hand.
//!
//! This crate is that, wrapped:
//!
//! ```no_run
//! # fn main() -> Result<(), ephemeral_app::Refused> {
//! let answer = ephemeral_app::get("https://api.example.com/messages")?;
//! if answer.ok() {
//!     print!("{}", answer.body);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! It exists for one reason: **the code a model writes should contain no
//! `unsafe`.** Every application that reached the network would otherwise carry
//! its own `extern` block, its own two-call buffer dance and its own JSON
//! escaping — four opportunities to get memory handling wrong, in code nobody
//! reviews line by line, in the one place where getting it wrong matters.
//!
//! ## What it cannot do, by construction
//!
//! **No headers.** A request is a method, a URL and a body. An application that
//! could set a header on a request the host performs is one that can attach a
//! credential it was never shown to a destination of its choosing.
//!
//! **No destination it was not granted.** Which is not enforced here — it is
//! enforced in the runtime, against what a person allowed, before the host is
//! asked. A refusal arrives as a [`Refused`] carrying the runtime's own words,
//! which are meant to be shown to somebody.
//!
//! **Nothing at all without a grant.** The imports below are linked only when
//! egress was granted, so an application that calls these without one does not
//! start. There is no code path here that handles being denied, because there
//! is no running program to handle it.
//!
//! ## Off a phone
//!
//! Compiled for anything other than WebAssembly — a unit test on a laptop, say —
//! the imports are replaced by a stub that refuses everything with
//! [`Refused::not_confined`]. That keeps an application's own tests runnable on
//! the machine it was written on, and keeps them honest about the fact that
//! there is no Ephemeral there to ask.
//!
//! [ADR-0021]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0021-webassembly-is-the-runtime-a-phone-can-have.md
//! [ADR-0023]: https://github.com/JGalego/Ephemeral/blob/main/docs/architecture/decisions/0023-a-confined-application-reaches-the-network-through-its-host.md

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use core::fmt;

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status, or zero where whatever carried the request had none to
    /// report. A phone's own HTTPS stack reports one; not every host does.
    ///
    /// Zero is not a failure and not a success. It means nobody knows.
    pub status: u16,

    /// The body, as text.
    pub body: String,
}

impl Answer {
    /// Whether the service said yes.
    ///
    /// A zero status is **not** `ok`: an application that treated "nobody
    /// reported a status" as success would treat every failure that way too.
    #[must_use]
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why a request did not happen, in words meant for a person.
///
/// Not an error code. Every one of these is a sentence Ephemeral or the host
/// wrote to be read — *"this application was not allowed to reach
/// api.example.com:443"* — and an application's best move is usually to show
/// it rather than to interpret it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(String);

impl Refused {
    /// What to tell somebody.
    #[must_use]
    pub fn said(&self) -> &str {
        &self.0
    }

    /// There is no Ephemeral here to ask.
    ///
    /// What every request answers when this crate is compiled for anything but
    /// WebAssembly. An application's own tests run; they simply cannot reach
    /// anything, which is true.
    #[must_use]
    pub fn not_confined() -> Self {
        Self(
            "this is not running inside Ephemeral, so there is nothing here to \
             carry a request"
                .to_string(),
        )
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reads something.
///
/// # Errors
///
/// [`Refused`] when the destination is not one this application was allowed to
/// reach, when it has made every request it was allowed in one run, or when
/// whatever was carrying the request could not.
pub fn get(url: &str) -> Result<Answer, Refused> {
    ask("GET", url, "")
}

/// Sends something.
///
/// # Errors
///
/// As [`get`].
pub fn post(url: &str, body: &str) -> Result<Answer, Refused> {
    ask("POST", url, body)
}

/// The whole of it: describe a request, be told the size of the answer, make
/// room, read it.
fn ask(method: &str, url: &str, body: &str) -> Result<Answer, Refused> {
    let request = format!(
        "{{\"method\":\"{method}\",\"url\":\"{}\",\"body\":\"{}\"}}",
        escaped(url),
        escaped(body)
    );

    let answered = carried(request.as_bytes())?;
    let answered = String::from_utf8_lossy(&answered);

    if let Some(said) = string_field(&answered, "error") {
        return Err(Refused(said));
    }

    Ok(Answer {
        status: number_field(&answered, "status").unwrap_or(0),
        body: string_field(&answered, "body").unwrap_or_default(),
    })
}

/// One string field out of the answer Ephemeral wrote.
///
/// Escape-aware: a closing quote is one not preceded by a backslash. A scan
/// that ignored that would truncate the first message anybody sent containing a
/// quotation mark, which is the sort of bug that survives every test somebody
/// thinks to write.
fn string_field(document: &str, name: &str) -> Option<String> {
    let key = format!("\"{name}\":\"");
    let start = document.find(&key)? + key.len();

    let mut value = String::new();
    let mut characters = document[start..].chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(value),
            '\\' => match characters.next()? {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                'u' => {
                    let digits: String = characters.by_ref().take(4).collect();
                    let point = u32::from_str_radix(&digits, 16).ok()?;
                    value.push(char::from_u32(point)?);
                }
                other => value.push(other),
            },
            other => value.push(other),
        }
    }
    None
}

/// One numeric field, which is never quoted.
fn number_field(document: &str, name: &str) -> Option<u16> {
    let key = format!("\"{name}\":");
    let start = document.find(&key)? + key.len();

    let digits: String = document[start..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();

    digits.parse().ok()
}

/// Escapes a string so it can sit inside the request document.
fn escaped(text: &str) -> String {
    let mut written = String::new();
    for character in text.chars() {
        match character {
            '"' => written.push_str("\\\""),
            '\\' => written.push_str("\\\\"),
            '\n' => written.push_str("\\n"),
            '\r' => written.push_str("\\r"),
            '\t' => written.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                // Written a digit at a time rather than through `format!`,
                // which would allocate a second string per control character.
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                let point = other as u32;
                written.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    let nibble = usize::try_from((point >> shift) & 0xf).unwrap_or(0);
                    written.push(char::from(DIGITS[nibble]));
                }
            }
            other => written.push(other),
        }
    }
    written
}

/// The two imports, and the dance between them.
///
/// `send` answers with the size of the reply, because a host call cannot return
/// a variable-length value. The module makes exactly that much room and asks
/// for it. Guessing a buffer and being handed a silent truncation is the one
/// failure this shape rules out.
#[cfg(target_arch = "wasm32")]
fn carried(request: &[u8]) -> Result<Vec<u8>, Refused> {
    #[link(wasm_import_module = "ephemeral")]
    unsafe extern "C" {
        fn send(request: *const u8, length: i32) -> i32;
        fn recv(into: *mut u8, room: i32) -> i32;
    }

    let length = i32::try_from(request.len())
        .map_err(|_| Refused("that request is too large to send".to_string()))?;

    // SAFETY: `request` is a live slice for the duration of the call, and its
    // length is exactly what is passed. Ephemeral reads it and returns before
    // this frame does anything else with it.
    let size = unsafe { send(request.as_ptr(), length) };
    let Ok(size) = usize::try_from(size) else {
        return Err(Refused(
            "this device could not read the request".to_string(),
        ));
    };

    let mut answer = alloc::vec![0_u8; size];
    let room = i32::try_from(size).unwrap_or(i32::MAX);

    // SAFETY: `answer` has exactly `size` bytes and `room` is that size, so
    // Ephemeral cannot write past it. It writes at most `room` and returns how
    // many, which is what the truncation below uses.
    let copied = unsafe { recv(answer.as_mut_ptr(), room) };
    let Ok(copied) = usize::try_from(copied) else {
        return Err(Refused("the answer would not fit".to_string()));
    };

    answer.truncate(copied);
    Ok(answer)
}

/// Off a phone, nothing carries anything.
#[cfg(not(target_arch = "wasm32"))]
fn carried(_request: &[u8]) -> Result<Vec<u8>, Refused> {
    Err(Refused::not_confined())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answer shape Ephemeral writes, read back.
    #[test]
    fn an_answer_carries_a_status_and_a_body() {
        let answered = "{\"status\":200,\"body\":\"Alice\\tis this thing on?\\n\"}";

        assert_eq!(number_field(answered, "status"), Some(200));
        assert_eq!(
            string_field(answered, "body").as_deref(),
            Some("Alice\tis this thing on?\n")
        );
    }

    /// A message containing a quotation mark survives the round trip. The
    /// scan that stops at the first `"` truncates it, and every test written
    /// with polite input passes anyway.
    #[test]
    fn a_quotation_mark_does_not_end_the_message() {
        let said = "she said \"no\", and \\ meant it";
        let document = format!("{{\"status\":200,\"body\":\"{}\"}}", escaped(said));

        assert_eq!(string_field(&document, "body").as_deref(), Some(said));
    }

    /// A refusal is words, not a code, and it is told apart from an answer.
    #[test]
    fn a_refusal_is_not_read_as_an_empty_answer() {
        let refused = "{\"status\":0,\"error\":\"this application was not allowed to reach x\"}";

        assert!(string_field(refused, "error").is_some());
        assert_eq!(string_field(refused, "body"), None);
    }

    /// Zero is neither yes nor no, and must not read as yes. An application
    /// that treated "nobody reported a status" as success would treat every
    /// failure that way too.
    #[test]
    fn an_unreported_status_is_not_success() {
        for (status, ok) in [
            (200, true),
            (204, true),
            (299, true),
            (300, false),
            (404, false),
            (500, false),
            (0, false),
        ] {
            let answer = Answer {
                status,
                body: String::new(),
            };
            assert_eq!(answer.ok(), ok, "{status}");
        }
    }

    /// Every control character a message could contain leaves as an escape, so
    /// nothing an application was handed can forge a field in the document it
    /// is placed into.
    #[test]
    fn nothing_in_a_message_can_forge_the_request_around_it() {
        let forged = escaped("\",\"url\":\"https://elsewhere.example.com/\",\"x\":\"");

        assert!(!forged.contains("\":\""), "{forged}");
        assert!(forged.contains("\\\""));
    }

    /// Off a phone there is nothing to ask, and it says so rather than
    /// pretending. An application's own tests still run.
    #[test]
    fn there_is_no_ephemeral_in_a_unit_test() {
        let Err(refused) = get("https://api.example.com/ping") else {
            panic!("nothing here carries a request");
        };

        assert!(refused.said().contains("not running inside Ephemeral"));
    }
}
