//! A two-party messaging application, over a network it never touches.
//!
//!     --as Alice --relay http://127.0.0.1:8787 --room garden --send "hello"
//!     --as Bob   --relay http://127.0.0.1:8787 --room garden --read
//!
//! There is no socket here and no folder either, and no `unsafe` — the crate
//! this links against holds the one `extern` block there is.

use std::fmt::Write as _;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match run(&arguments) {
        Ok(said) => print!("{said}"),
        Err(said) => {
            eprintln!("{said}");
            std::process::exit(1);
        }
    }
}

struct Asked {
    who: String,
    relay: String,
    room: String,
    send: Option<String>,
    page: bool,
}

impl Asked {
    fn from(arguments: &[String]) -> Result<Self, String> {
        let (mut who, mut relay, mut room, mut send) = (None, None, None, None);
        let (mut page, mut reading) = (false, false);

        let mut rest = arguments.iter();
        while let Some(argument) = rest.next() {
            let mut next = |what: &str| {
                rest.next()
                    .ok_or_else(|| format!("{what} needs a value after it"))
                    .cloned()
            };
            match argument.as_str() {
                "--as" => who = Some(next("--as")?),
                "--relay" => relay = Some(next("--relay")?),
                "--room" => room = Some(next("--room")?),
                "--send" => send = Some(next("--send")?),
                "--read" => reading = true,
                "--format" => page = next("--format")? == "html",
                other => return Err(format!("I do not know what {other} means")),
            }
        }

        let send = send.filter(|message| !message.trim().is_empty());
        if send.is_none() && !reading {
            return Err("say --send \"…\" to write something, or --read to catch up".to_owned());
        }

        Ok(Self {
            who: who.ok_or("--as needs a name, or nobody knows who is talking")?,
            relay: relay.ok_or("--relay needs the address you both meet at")?,
            room: room.ok_or("--room needs the name you both agreed on")?,
            send,
            page,
        })
    }
}

fn run(arguments: &[String]) -> Result<String, String> {
    let asked = Asked::from(arguments)?;
    let url = format!("{}/room/{}", asked.relay.trim_end_matches('/'), asked.room);

    // The whole of the network, and every refusal it can produce, in four
    // lines. A refusal is a sentence somebody wrote to be read, so it is shown
    // rather than interpreted.
    let answer = match &asked.send {
        Some(message) => {
            let line = format!(
                "{}\t{}",
                asked.who,
                message.replace(['\t', '\n'], " ").trim()
            );
            ephemeral_app::post(&url, &line)
        }
        None => ephemeral_app::get(&url),
    }
    .map_err(|refused| refused.said().to_owned())?;

    if !answer.ok() {
        return Err(format!(
            "the room answered {} rather than carrying that.\n{}",
            answer.status, answer.body
        ));
    }

    let messages: Vec<Message> = answer.body.lines().filter_map(Message::of).collect();

    Ok(if asked.page {
        page(&messages, &asked.who)
    } else {
        lines(&messages, &asked.who)
    })
}

struct Message {
    who: String,
    said: String,
}

impl Message {
    fn of(line: &str) -> Option<Self> {
        let (who, said) = line.split_once('\t')?;
        Some(Self {
            who: who.to_owned(),
            said: said.to_owned(),
        })
    }
}

fn lines(messages: &[Message], reader: &str) -> String {
    if messages.is_empty() {
        return "Nobody has said anything yet.\n".to_owned();
    }

    let mut said = String::new();
    for message in messages {
        let mine = if message.who == reader {
            "you"
        } else {
            &message.who
        };
        let _ = writeln!(said, "{mine}: {}", message.said);
    }
    said
}

/// The conversation as a page: no script, no image, nothing fetched.
fn page(messages: &[Message], reader: &str) -> String {
    let mut bubbles = String::new();
    for message in messages {
        let _ = writeln!(
            bubbles,
            "<li class=\"{}\"><b>{}</b>{}</li>",
            if message.who == reader {
                "mine"
            } else {
                "theirs"
            },
            markup(&message.who),
            markup(&message.said)
        );
    }
    if messages.is_empty() {
        bubbles.push_str("<li class=\"empty\">Nobody has said anything yet.</li>\n");
    }

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\"><head><meta charset=\"utf-8\">\n\
         <title>{reader}</title>\n\
         <style>\n\
         body {{ font: 16px system-ui, sans-serif; margin: 0; padding: 16px;\n\
                background: #101014; color: #eee; }}\n\
         ul {{ list-style: none; margin: 0; padding: 0; }}\n\
         li {{ max-width: 76%; margin: 8px 0; padding: 10px 14px; border-radius: 16px; }}\n\
         li b {{ display: block; font-size: 12px; opacity: .65; font-weight: 600; }}\n\
         .mine {{ margin-left: auto; background: #3b2f6b; border-bottom-right-radius: 4px; }}\n\
         .theirs {{ background: #24242c; border-bottom-left-radius: 4px; }}\n\
         .empty {{ opacity: .6; text-align: center; background: none; }}\n\
         </style></head><body>\n\
         <ul>\n{bubbles}</ul>\n\
         </body></html>\n",
        reader = markup(reader)
    )
}

/// Escapes what somebody else wrote before it reaches a renderer.
fn markup(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}
