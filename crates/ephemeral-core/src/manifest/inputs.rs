//! What an application says it takes, so something can ask for it.
//!
//! ## Why this exists
//!
//! Eight applications were generated from eight different sentences by a strong
//! model, and every one of them came back as a command-line tool with flags:
//!
//! ```text
//! csvdiff.py [--key KEYS] [--delimiter DELIMITER] [--no-header]
//!            [--output {plain,json}] old new
//! ```
//!
//! That is a form. Nobody should have to type it, and on a phone nobody
//! *can* — there is no terminal to type it into. The alternative usually
//! reached for is to make every application render its own screen, which asks a
//! model to write a user interface as well as a program, and doubles what can
//! go wrong for the large majority of these things that are one input, one
//! output and a couple of options.
//!
//! So an application declares its shape instead, and whatever is showing it —
//! a phone, a window, a terminal — draws the form. One implementation per
//! client rather than one per application, and the application stays a program.
//!
//! ## The rule this follows
//!
//! **A declaration is not a permission.** An application saying it takes a file
//! is not an application that may read that file; the person still chooses
//! which one, and the sandbox still only contains what was granted. Declaring
//! an input widens nothing, which is what makes it safe for a model to write.

use serde::{Deserialize, Serialize};

/// One thing an application needs before it can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// What the application calls it. Also the form field's identity.
    pub name: String,

    /// What to call it on screen, in a person's words.
    pub label: String,

    /// What sort of value it is, which decides what the form shows.
    pub kind: InputKind,

    /// How the value reaches the application.
    pub passing: Passing,

    /// Whether the application cannot run without it.
    #[serde(default)]
    pub required: bool,

    /// What it is when nobody chooses. `None` means the application decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// One line of help, if the application offered one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// What sort of value an input holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputKind {
    /// Free text.
    Text,

    /// A number.
    Number,

    /// A file to work on.
    ///
    /// A client offers a picker limited to what the application may actually
    /// see — a file it was never granted is not a file it can be given, so
    /// offering one would be offering a choice that fails on use.
    File,

    /// A folder to work on. The same rule as [`InputKind::File`].
    Folder,

    /// One of a fixed set.
    Choice {
        /// What may be chosen. A value outside this set is refused rather than
        /// passed through, because the set is the application's own claim about
        /// what it understands.
        options: Vec<String>,
    },

    /// On or off. Passed by being present rather than by having a value.
    Flag,
}

/// How a value reaches the application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "passing", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Passing {
    /// In order, with no name: `app old new`.
    Positional {
        /// Where in the order, counting from zero.
        at: u8,
    },

    /// Named: `app --key id`.
    Named {
        /// The flag, exactly as the application expects it.
        flag: String,
    },
}

/// Why a set of answers cannot be turned into a command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InputError {
    /// A required input was left empty.
    #[error("{label} is needed before this can run")]
    Missing {
        /// Which one, in the words shown on screen.
        label: String,
    },

    /// A choice that is not one of the offered ones.
    #[error("{label} cannot be {given:?} — it is one of: {offered}")]
    NotOffered {
        /// Which one.
        label: String,
        /// What was asked for.
        given: String,
        /// What is available.
        offered: String,
    },

    /// A flag that is not shaped like one.
    ///
    /// The flag comes from the application's own declaration, which a model
    /// wrote. A "flag" that is really a value would silently become a
    /// positional argument in the wrong place.
    #[error("{name} declares {flag:?} as a flag, which is not one")]
    NotAFlag {
        /// The input that declared it.
        name: String,
        /// What it declared.
        flag: String,
    },
}

/// Turns what somebody filled in into the arguments an application receives.
///
/// This lives here, in the domain, rather than in each client, so that a phone,
/// a window and a terminal build the same command from the same answers. Three
/// implementations of this would be three subtly different applications.
///
/// Nothing here is shell-parsed and nothing is quoted, because the result is an
/// argument vector rather than a command line — the same reason
/// [`crate::manifest::RuntimeSpec::entrypoint`] is a vector.
///
/// # Errors
///
/// [`InputError`] naming the input and what is wrong with it, in words meant
/// for the person who filled the form in.
pub fn arguments(
    inputs: &[Input],
    answers: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<String>, InputError> {
    let mut positional: Vec<(u8, String)> = Vec::new();
    let mut named: Vec<String> = Vec::new();

    for input in inputs {
        let given = answers
            .get(&input.name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .or(input.default.as_deref());

        let Some(value) = given else {
            if input.required {
                return Err(InputError::Missing {
                    label: input.label.clone(),
                });
            }
            continue;
        };

        if let InputKind::Choice { options } = &input.kind
            && !options.iter().any(|option| option == value)
        {
            return Err(InputError::NotOffered {
                label: input.label.clone(),
                given: value.to_owned(),
                offered: options.join(", "),
            });
        }

        match &input.passing {
            Passing::Positional { at } => positional.push((*at, value.to_owned())),
            Passing::Named { flag } => {
                if !flag.starts_with('-') {
                    return Err(InputError::NotAFlag {
                        name: input.name.clone(),
                        flag: flag.clone(),
                    });
                }

                if input.kind == InputKind::Flag {
                    // A flag is on by being there and off by being absent, so
                    // its *value* decides whether to write it at all.
                    //
                    // Found by running this against a real model: gpt-5
                    // declared `--no-headers` with `"default": "false"`, which
                    // is a perfectly reasonable way to say "off by default" and
                    // which the first version of this turned into a flag that
                    // was always on. A checkbox nobody ticked would have
                    // silently told the application there were no headers.
                    if switched_on(value) {
                        named.push(flag.clone());
                    }
                    continue;
                }

                named.push(flag.clone());
                named.push(value.to_owned());
            }
        }
    }

    // Named first, positional after, in declared order. Every argument parser
    // worth the name accepts that, and the reverse trips the ones that stop
    // looking for options after the first positional.
    positional.sort_by_key(|(at, _)| *at);
    named.extend(positional.into_iter().map(|(_, value)| value));

    Ok(named)
}

/// Whether a flag input is switched on.
///
/// A checkbox produces "true" or "false", and "false" must not put the flag on
/// the command line — an absent flag is how "off" is expressed.
#[must_use]
pub fn switched_on(value: &str) -> bool {
    matches!(value, "true" | "yes" | "on" | "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn answers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn positional(name: &str, at: u8, required: bool) -> Input {
        Input {
            name: name.to_owned(),
            label: name.to_owned(),
            kind: InputKind::File,
            passing: Passing::Positional { at },
            required,
            default: None,
            help: None,
        }
    }

    fn named(name: &str, flag: &str, kind: InputKind) -> Input {
        Input {
            name: name.to_owned(),
            label: name.to_owned(),
            kind,
            passing: Passing::Named {
                flag: flag.to_owned(),
            },
            required: false,
            default: None,
            help: None,
        }
    }

    /// The application that prompted all of this, as a form.
    #[test]
    fn the_csv_comparator_becomes_a_command() {
        let inputs = vec![
            positional("old", 0, true),
            positional("new", 1, true),
            named("key", "--key", InputKind::Text),
            named("no_header", "--no-header", InputKind::Flag),
        ];

        let built = arguments(
            &inputs,
            &answers(&[
                ("old", "/data/before.csv"),
                ("new", "/data/after.csv"),
                ("key", "id"),
            ]),
        )
        .expect("a complete form");

        assert_eq!(
            built,
            ["--key", "id", "/data/before.csv", "/data/after.csv"],
            "options first, then positionals in declared order"
        );
    }

    /// A flag that is on appears; a flag that is off does not. Passing `false`
    /// after a flag would hand the application a positional argument it never
    /// asked for.
    #[test]
    fn a_flag_is_present_or_absent_rather_than_true_or_false() {
        let inputs = vec![named("no_header", "--no-header", InputKind::Flag)];

        let on = arguments(&inputs, &answers(&[("no_header", "true")])).expect("on");
        assert_eq!(on, ["--no-header"], "and carries no value");

        let off = arguments(&inputs, &answers(&[("no_header", "")])).expect("off");
        assert!(off.is_empty(), "off is absence, not a value");

        let unticked = arguments(&inputs, &answers(&[("no_header", "false")])).expect("off");
        assert!(unticked.is_empty(), "and an explicit no is still off");
    }

    /// A flag whose default is `"false"`, exactly as a real model declared one.
    ///
    /// This is the shape that found the bug. gpt-5, asked for a CSV comparator,
    /// declared `--no-headers` with `"default": "false"` — a perfectly
    /// reasonable way to write "off unless asked". The first version of this
    /// treated any non-empty default as a value to use, so the flag was always
    /// on, and a checkbox nobody ticked would have told the application there
    /// were no headers.
    #[test]
    fn a_flag_defaulting_to_false_is_off() {
        let mut input = named("no_headers", "--no-headers", InputKind::Flag);
        input.default = Some("false".to_owned());

        assert!(
            arguments(std::slice::from_ref(&input), &answers(&[]))
                .expect("it builds")
                .is_empty(),
            "a default of false means the flag is not passed"
        );

        assert_eq!(
            arguments(&[input], &answers(&[("no_headers", "true")])).expect("it builds"),
            ["--no-headers"],
            "and ticking it still turns it on"
        );
    }

    /// An empty answer for something required is a question to ask, not a
    /// command to run.
    #[test]
    fn a_required_input_left_empty_is_refused_by_name() {
        let inputs = vec![positional("old", 0, true)];

        let refused = arguments(&inputs, &answers(&[])).expect_err("it cannot run");

        assert_eq!(
            refused,
            InputError::Missing {
                label: "old".to_owned()
            }
        );
        assert!(refused.to_string().contains("old"));
    }

    /// A default fills in for an answer nobody gave, and an answer overrides a
    /// default.
    #[test]
    fn a_default_stands_in_until_somebody_chooses() {
        let mut input = named("delimiter", "--delimiter", InputKind::Text);
        input.default = Some(",".to_owned());

        assert_eq!(
            arguments(std::slice::from_ref(&input), &answers(&[])).expect("the default"),
            ["--delimiter", ","]
        );
        assert_eq!(
            arguments(&[input], &answers(&[("delimiter", ";")])).expect("the answer"),
            ["--delimiter", ";"]
        );
    }

    /// The offered set is the application's own claim about what it
    /// understands, so a value outside it is refused here rather than passed
    /// through to fail somewhere less legible.
    #[test]
    fn a_choice_outside_what_is_offered_is_refused() {
        let inputs = vec![named(
            "output",
            "--output",
            InputKind::Choice {
                options: vec!["plain".to_owned(), "json".to_owned()],
            },
        )];

        let refused =
            arguments(&inputs, &answers(&[("output", "yaml")])).expect_err("not on offer");

        assert!(refused.to_string().contains("plain, json"), "{refused}");
    }

    /// The flag comes from a declaration a model wrote. One that is not shaped
    /// like a flag would quietly become a positional argument in the wrong
    /// place, which is a worse failure than refusing.
    #[test]
    fn a_flag_that_is_not_a_flag_is_refused() {
        let inputs = vec![named("output", "output", InputKind::Text)];

        let refused = arguments(&inputs, &answers(&[("output", "json")])).expect_err("not a flag");

        assert!(matches!(refused, InputError::NotAFlag { .. }), "{refused}");
    }

    /// Positional order is what the application declared, not the order
    /// somebody happened to fill the form in.
    #[test]
    fn positionals_keep_the_order_the_application_declared() {
        let inputs = vec![positional("second", 1, true), positional("first", 0, true)];

        let built = arguments(&inputs, &answers(&[("first", "a"), ("second", "b")]))
            .expect("a complete form");

        assert_eq!(built, ["a", "b"]);
    }

    /// Declaring an input is not asking for a permission. Nothing here can
    /// widen what an application may reach, which is what makes it safe for a
    /// model to write.
    #[test]
    fn a_declaration_grants_nothing() {
        let inputs = vec![positional("secrets", 0, true)];

        let built = arguments(&inputs, &answers(&[("secrets", "/etc/shadow")]))
            .expect("it builds a command");

        // The path is carried verbatim. Whether the application can open it is
        // the sandbox's answer, and the sandbox contains only what was granted.
        assert_eq!(built, ["/etc/shadow"]);
    }

    /// The whole set survives being written down and read back, because a
    /// manifest is a file on disk that outlives the process that wrote it.
    #[test]
    fn a_declaration_survives_the_round_trip() {
        let inputs = vec![
            positional("old", 0, true),
            named(
                "output",
                "--output",
                InputKind::Choice {
                    options: vec!["plain".to_owned()],
                },
            ),
        ];

        let written = serde_json::to_string(&inputs).expect("it serialises");
        let read: Vec<Input> = serde_json::from_str(&written).expect("it parses back");

        assert_eq!(read, inputs);
    }
}
