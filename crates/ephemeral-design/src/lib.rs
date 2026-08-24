//! The colours Ephemeral is drawn in, and the rules they have to meet.
//!
//! There are two clients and there will be three. A palette written out by hand
//! in each of them is a palette that disagrees with itself within a month —
//! which is the same failure the service layer exists to prevent, one layer
//! further out: the terminal and the window must not describe a critical
//! permission in two different reds any more than they may describe it in two
//! different sentences.
//!
//! So the palette lives here once, and every platform's file is generated from
//! it ([`css`], [`android`]). The generated files are checked in — a build step
//! for a window that has none would be a poor trade — and a test asserts they
//! still match, so editing one by hand fails rather than drifts.
//!
//! The other half is that a colour is not a decoration here. Risk is carried by
//! colour in both clients, so a risk nobody can read is a permission prompt
//! that does not work. Every pairing this palette actually uses is checked
//! against [WCAG 2.1 contrast][wcag] in `tests/contrast.rs`, which is the
//! difference between a palette that is accessible and one that is described
//! as accessible.
//!
//! [wcag]: https://www.w3.org/TR/WCAG21/#contrast-minimum

// Tests here assert against constants in this same file, so a failure is a typo
// in the palette rather than bad input, and panicking says so most clearly.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::fmt::Write as _;

/// One colour, and what it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// Its name, which is the same on every platform: `ink`, `ground`, `high`.
    pub name: &'static str,

    /// `#rrggbb`, lower case.
    pub hex: &'static str,

    /// What it is for, in a sentence. Emitted as a comment, because the next
    /// person choosing a colour is choosing from this list.
    pub what: &'static str,
}

/// A whole palette: every colour, in one lighting.
#[derive(Debug, Clone, Copy)]
pub struct Scheme {
    /// `dark` or `light`.
    pub name: &'static str,

    /// Every colour in it. Both schemes carry the same names, so anything
    /// drawn in one is drawable in the other.
    pub colours: &'static [Colour],
}

impl Scheme {
    /// The hex for a name, if the scheme has one by that name.
    ///
    /// Both schemes are constants and a test asserts they carry the same names,
    /// so `None` here means a typo in this crate — but it is still `None`
    /// rather than a panic, because a generator that aborts halfway through
    /// leaves half a stylesheet on disk.
    #[must_use]
    pub fn hex(&self, name: &str) -> Option<&'static str> {
        self.colours
            .iter()
            .find(|colour| colour.name == name)
            .map(|colour| colour.hex)
    }
}

/// What Ephemeral looks like when nobody has said otherwise.
///
/// Dark is the default rather than the alternative, and not by media query: a
/// browser reports `light` for a machine that has expressed no preference at
/// all, so following the preference means being white for everybody who never
/// set one. This is a tool for watching what software on your machine is
/// allowed to do, often for a minute at a time, and a window that flashes white
/// at somebody is a window they close. Light is one click away and is
/// remembered.
pub const DARK: Scheme = Scheme {
    name: "dark",
    colours: &[
        Colour {
            name: "ground",
            hex: "#0b0e17",
            what: "the page itself",
        },
        Colour {
            name: "ground-soft",
            hex: "#10141f",
            what: "the page, one step lighter: gradients and inset areas",
        },
        Colour {
            name: "paper",
            hex: "#ffffff",
            what: "somebody else's document, drawn on its own paper — white in \
                   both schemes, because a generated page does not know which \
                   one is in force and will use a browser's own black text",
        },
        Colour {
            name: "surface",
            hex: "#151a28",
            what: "a card sitting on the page",
        },
        Colour {
            name: "surface-high",
            hex: "#1d2334",
            what: "something floating over it: a banner, a field, a menu",
        },
        Colour {
            name: "edge",
            hex: "#283149",
            what: "a hairline that separates without drawing attention",
        },
        Colour {
            name: "edge-strong",
            hex: "#626f90",
            what: "the outline of something you can operate",
        },
        Colour {
            name: "ink",
            hex: "#e9ecf5",
            what: "what you are meant to read",
        },
        Colour {
            name: "ink-quiet",
            hex: "#a6afc6",
            what: "supporting text: reasons, timestamps, counts",
        },
        Colour {
            name: "ink-faint",
            hex: "#8891a8",
            what: "the quietest text that is still text",
        },
        Colour {
            name: "accent",
            hex: "#a78bfa",
            what: "Ephemeral itself: the primary action, and every focus ring",
        },
        Colour {
            name: "accent-ink",
            hex: "#0b0e17",
            what: "text on top of the accent",
        },
        Colour {
            name: "accent-soft",
            hex: "#241e45",
            what: "a ground tinted towards the accent",
        },
        Colour {
            name: "low",
            hex: "#4fd69c",
            what: "low risk, and things that went well",
        },
        Colour {
            name: "low-soft",
            hex: "#102820",
            what: "a ground tinted towards low risk",
        },
        Colour {
            name: "medium",
            hex: "#f0b44e",
            what: "medium risk, and anything that needs care but is not wrong",
        },
        Colour {
            name: "medium-soft",
            hex: "#2a2110",
            what: "a ground tinted towards medium risk",
        },
        Colour {
            name: "high",
            hex: "#ff9057",
            what: "high risk, and failures",
        },
        Colour {
            name: "high-soft",
            hex: "#2e1a12",
            what: "a ground tinted towards high risk",
        },
        Colour {
            name: "critical",
            hex: "#ff7a8a",
            what: "the permissions that can do the most damage",
        },
        Colour {
            name: "critical-soft",
            hex: "#301419",
            what: "a ground tinted towards critical risk",
        },
    ],
};

/// The same palette for a machine whose owner has asked for light.
///
/// Not an afterthought and not an inversion: the risks have to stay in the same
/// order of alarm against a pale ground, which is a different set of colours
/// rather than the same ones lightened.
pub const LIGHT: Scheme = Scheme {
    name: "light",
    colours: &[
        Colour {
            name: "ground",
            hex: "#f4f6fb",
            what: "the page itself",
        },
        Colour {
            name: "ground-soft",
            hex: "#ffffff",
            what: "the page, one step lighter: gradients and inset areas",
        },
        Colour {
            name: "paper",
            hex: "#ffffff",
            what: "somebody else's document, drawn on its own paper — white in \
                   both schemes, because a generated page does not know which \
                   one is in force and will use a browser's own black text",
        },
        Colour {
            name: "surface",
            hex: "#ffffff",
            what: "a card sitting on the page",
        },
        Colour {
            name: "surface-high",
            hex: "#ffffff",
            what: "something floating over it: a banner, a field, a menu",
        },
        Colour {
            name: "edge",
            hex: "#e2e6f0",
            what: "a hairline that separates without drawing attention",
        },
        Colour {
            name: "edge-strong",
            hex: "#828da7",
            what: "the outline of something you can operate",
        },
        Colour {
            name: "ink",
            hex: "#131728",
            what: "what you are meant to read",
        },
        Colour {
            name: "ink-quiet",
            hex: "#525a74",
            what: "supporting text: reasons, timestamps, counts",
        },
        Colour {
            name: "ink-faint",
            hex: "#626a84",
            what: "the quietest text that is still text",
        },
        Colour {
            name: "accent",
            hex: "#5b3dd4",
            what: "Ephemeral itself: the primary action, and every focus ring",
        },
        Colour {
            name: "accent-ink",
            hex: "#ffffff",
            what: "text on top of the accent",
        },
        Colour {
            name: "accent-soft",
            hex: "#efebff",
            what: "a ground tinted towards the accent",
        },
        Colour {
            name: "low",
            hex: "#0f6a46",
            what: "low risk, and things that went well",
        },
        Colour {
            name: "low-soft",
            hex: "#e7f5ee",
            what: "a ground tinted towards low risk",
        },
        Colour {
            name: "medium",
            hex: "#7d5207",
            what: "medium risk, and anything that needs care but is not wrong",
        },
        Colour {
            name: "medium-soft",
            hex: "#fbf1de",
            what: "a ground tinted towards medium risk",
        },
        Colour {
            name: "high",
            hex: "#a3380e",
            what: "high risk, and failures",
        },
        Colour {
            name: "high-soft",
            hex: "#fceee7",
            what: "a ground tinted towards high risk",
        },
        Colour {
            name: "critical",
            hex: "#93122e",
            what: "the permissions that can do the most damage",
        },
        Colour {
            name: "critical-soft",
            hex: "#fbeaee",
            what: "a ground tinted towards critical risk",
        },
    ],
};

/// Two colours that end up on top of each other, and how far apart they have
/// to be.
#[derive(Debug, Clone, Copy)]
pub struct Pairing {
    /// The colour in front.
    pub fore: &'static str,

    /// The colour behind it.
    pub back: &'static str,

    /// The contrast ratio this pairing must reach.
    ///
    /// 4.5 is WCAG AA for text at ordinary sizes. 3.0 is what a control's
    /// outline needs — it carries no words, only the fact that it is there.
    pub least: f64,

    /// What is drawn this way, so a failure names the screen rather than the
    /// hex.
    pub what: &'static str,
}

/// Every pairing the two clients actually draw.
///
/// A palette is not accessible; a *use* of one is. This list is the contract:
/// anything drawn in Ephemeral has to appear here, and everything here is
/// checked in both schemes on every commit.
pub const PAIRINGS: &[Pairing] = &[
    Pairing {
        fore: "ink",
        back: "ground",
        least: 7.0,
        what: "body text on the page",
    },
    Pairing {
        fore: "ink",
        back: "surface",
        least: 7.0,
        what: "body text on a card",
    },
    Pairing {
        fore: "ink",
        back: "surface-high",
        least: 7.0,
        what: "body text in a banner or a field",
    },
    Pairing {
        fore: "ink-quiet",
        back: "ground",
        least: 4.5,
        what: "a reason or a timestamp on the page",
    },
    Pairing {
        fore: "ink-quiet",
        back: "surface",
        least: 4.5,
        what: "a reason or a timestamp on a card",
    },
    Pairing {
        fore: "ink-faint",
        back: "surface",
        least: 4.5,
        what: "the quietest text there is, which is still text",
    },
    Pairing {
        fore: "accent",
        back: "ground",
        least: 4.5,
        what: "a link, and the focus ring around whatever has the keyboard",
    },
    Pairing {
        fore: "accent",
        back: "surface",
        least: 4.5,
        what: "the same, on a card",
    },
    Pairing {
        fore: "accent-ink",
        back: "accent",
        least: 4.5,
        what: "the label on the primary button",
    },
    // The one thing on a list that shouts. It is filled rather than outlined,
    // so its text sits on a risk colour rather than on a ground — a pairing
    // this list did not have until somebody read the markup looking for one it
    // had missed.
    Pairing {
        fore: "ground",
        back: "high",
        least: 4.5,
        what: "\"2 decisions waiting\", which is drawn filled",
    },
    // A control's outline carries no words, so it needs 3:1 rather than 4.5 —
    // but it needs it on both grounds, because buttons sit on the page as well
    // as on cards. The first version of this palette failed here, in both
    // schemes, and this test is what said so.
    Pairing {
        fore: "edge-strong",
        back: "surface",
        least: 3.0,
        what: "the outline of a button on a card",
    },
    Pairing {
        fore: "edge-strong",
        back: "ground",
        least: 3.0,
        what: "the outline of a button on the page",
    },
    // Risk is carried by colour in both clients. A risk level nobody can read
    // is a permission prompt that does not work, so every level is checked
    // against every ground it is ever drawn on.
    Pairing {
        fore: "low",
        back: "ground",
        least: 4.5,
        what: "low risk, on the page",
    },
    Pairing {
        fore: "low",
        back: "surface",
        least: 4.5,
        what: "low risk, on a card",
    },
    Pairing {
        fore: "low",
        back: "low-soft",
        least: 4.5,
        what: "low risk on its own tinted ground",
    },
    Pairing {
        fore: "medium",
        back: "ground",
        least: 4.5,
        what: "medium risk, on the page",
    },
    Pairing {
        fore: "medium",
        back: "surface",
        least: 4.5,
        what: "medium risk, on a card",
    },
    Pairing {
        fore: "medium",
        back: "medium-soft",
        least: 4.5,
        what: "medium risk on its own tinted ground",
    },
    Pairing {
        fore: "high",
        back: "ground",
        least: 4.5,
        what: "high risk, on the page",
    },
    Pairing {
        fore: "high",
        back: "surface",
        least: 4.5,
        what: "high risk, on a card",
    },
    Pairing {
        fore: "high",
        back: "high-soft",
        least: 4.5,
        what: "high risk on its own tinted ground, which is the failure banner",
    },
    Pairing {
        fore: "critical",
        back: "ground",
        least: 4.5,
        what: "critical risk, on the page",
    },
    Pairing {
        fore: "critical",
        back: "surface",
        least: 4.5,
        what: "critical risk, on a card",
    },
    Pairing {
        fore: "critical",
        back: "critical-soft",
        least: 4.5,
        what: "critical risk on its own tinted ground",
    },
];

/// Both schemes, in the order a file should emit them.
pub const SCHEMES: &[Scheme] = &[DARK, LIGHT];

/// A colour as the numbers a contrast ratio is computed from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    /// Red, 0–255.
    pub red: u8,
    /// Green, 0–255.
    pub green: u8,
    /// Blue, 0–255.
    pub blue: u8,
}

impl Rgb {
    /// Reads `#rrggbb`.
    ///
    /// # Errors
    ///
    /// If it is not six hex digits behind a `#`.
    pub fn parse(hex: &str) -> Result<Self, String> {
        let digits = hex
            .strip_prefix('#')
            .ok_or_else(|| format!("{hex} does not start with #"))?;

        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{hex} is not six hex digits"));
        }

        let byte = |from: usize| -> u8 {
            u8::from_str_radix(&digits[from..from + 2], 16).unwrap_or_default()
        };

        Ok(Self {
            red: byte(0),
            green: byte(2),
            blue: byte(4),
        })
    }

    /// Relative luminance, as WCAG 2.1 defines it.
    #[must_use]
    pub fn luminance(self) -> f64 {
        fn channel(value: u8) -> f64 {
            let part = f64::from(value) / 255.0;
            if part <= 0.039_28 {
                part / 12.92
            } else {
                ((part + 0.055) / 1.055).powf(2.4)
            }
        }

        0.2126 * channel(self.red) + 0.7152 * channel(self.green) + 0.0722 * channel(self.blue)
    }
}

/// How far apart two colours are, 1.0 (identical) to 21.0 (black on white).
#[must_use]
pub fn contrast(fore: Rgb, back: Rgb) -> f64 {
    let (lighter, darker) = {
        let (a, b) = (fore.luminance(), back.luminance());
        if a > b { (a, b) } else { (b, a) }
    };

    (lighter + 0.05) / (darker + 0.05)
}

/// A banner saying not to edit the file this is at the top of.
///
/// One text, wrapped for whichever language the file is in. CSS has no nested
/// comments and XML has no `--` inside one, so the wrapping is per-language
/// rather than a prefix glued onto every line.
fn generated_by(open: &str, line: &str, close: &str) -> String {
    const SAID: &[&str] = &[
        "Generated by `cargo run -p ephemeral-design`. Do not edit.",
        "",
        "The palette lives in crates/ephemeral-design/src/lib.rs, once, because",
        "two clients holding their own copies of a risk colour is two clients",
        "that will eventually disagree about how alarming a permission is.",
        "Every pairing in it is checked for contrast by that crate's tests.",
    ];

    let mut out = String::from(open);
    out.push('\n');
    for said in SAID {
        if said.is_empty() {
            out.push_str(line.trim_end());
        } else {
            out.push_str(line);
            out.push_str(said);
        }
        out.push('\n');
    }
    out.push_str(close);
    out.push('\n');
    out
}

/// What is said above the light half of the stylesheet.
const LIGHT_PREAMBLE: &str = "
/* Light, for whoever asks for it, and only for them.
 *
 * Deliberately an attribute rather than `prefers-color-scheme`. A browser
 * reports `light` for a machine that has expressed no preference at all, so a
 * media query here would mean \"light unless the desktop says otherwise\" —
 * which is the opposite of the intent. Ephemeral is dark; somebody who wants
 * light says so once, and the window remembers. */
:root[data-theme=\"light\"] {
";

/// The desktop window's custom properties: dark by default, light when the
/// machine asks for light.
#[must_use]
pub fn css() -> String {
    let mut out = generated_by("/*", " * ", " */");

    out.push_str("\n:root {\n");
    for colour in DARK.colours {
        let _ = writeln!(
            out,
            "  /* {} */\n  --{}: {};",
            colour.what, colour.name, colour.hex
        );
    }
    out.push_str("}\n");

    out.push_str(LIGHT_PREAMBLE);
    for colour in LIGHT.colours {
        let _ = writeln!(out, "  --{}: {};", colour.name, colour.hex);
    }
    out.push_str("}\n");

    out
}

/// Android's colour resources.
///
/// One scheme, deliberately. `minSdk` here is 26, which predates the system
/// dark-mode setting the `-night` qualifier follows, so a light scheme on
/// Android would be chosen by a switch a good share of supported phones do not
/// have. The app is dark, and it is the same dark the window uses.
#[must_use]
pub fn android() -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str(&generated_by("<!--", "  ", "-->"));
    out.push_str("<resources>\n");

    for colour in DARK.colours {
        let _ = writeln!(
            out,
            "    <!-- {} -->\n    <color name=\"{}\">{}</color>",
            colour.what,
            colour.name.replace('-', "_"),
            colour.hex
        );
    }

    out.push_str("</resources>\n");
    out
}

/// The iOS application's colours.
///
/// SwiftUI takes components as fractions rather than bytes, so the conversion
/// happens here, once, rather than in a helper every view has to remember to
/// call. `Color(red:green:blue:)` is sRGB and needs no UIKit, which keeps this
/// compilable by anything that can compile SwiftUI.
#[must_use]
pub fn swift() -> String {
    let mut out = generated_by("//", "// ", "//");

    out.push_str("\nimport SwiftUI\n\n");
    out.push_str("/// Every colour Ephemeral draws, by the name it has on every platform.\n");
    out.push_str("enum Palette {\n");

    for colour in DARK.colours {
        let rgb = Rgb::parse(colour.hex).unwrap_or(Rgb {
            red: 0,
            green: 0,
            blue: 0,
        });

        let name = lower_camel(colour.name);
        let _ = writeln!(out, "    /// {}", colour.what);
        let _ = writeln!(
            out,
            "    static let {name} = Color(red: {:.4}, green: {:.4}, blue: {:.4})",
            f64::from(rgb.red) / 255.0,
            f64::from(rgb.green) / 255.0,
            f64::from(rgb.blue) / 255.0
        );
    }

    out.push_str(SWIFT_MAPPINGS);
    out
}

/// The two mappings every client needs, in Swift.
///
/// Kept as text rather than built up line by line: it is a fixed piece of
/// source, and a loop that emitted it would be harder to read than the thing
/// it emits.
const SWIFT_MAPPINGS: &str = r#"
    /// The colour a lifecycle state is drawn in, by the kind of state it is.
    /// The engine's own vocabulary, so a phone cannot quietly decide a state is
    /// calmer than the window thinks it is.
    static func forState(_ kind: String) -> Color {
        switch kind {
        case "working": return accent
        case "awaitinguser": return medium
        case "active": return low
        case "attention": return high
        case "archived", "deleted": return inkFaint
        default: return inkQuiet
        }
    }

    /// The colour of a risk level. An unrecognised one is ordinary text, never
    /// green: guessing "low" about the widest permission Ephemeral offers is
    /// the one guess that does real harm.
    static func forRisk(_ level: String?) -> Color {
        switch level {
        case "low": return low
        case "medium": return medium
        case "high": return high
        case "critical": return critical
        default: return ink
        }
    }
}
"#;

/// `ink-quiet` as `inkQuiet`, because Swift is not CSS.
fn lower_camel(name: &str) -> String {
    let mut out = String::new();
    let mut capitalise = false;

    for character in name.chars() {
        if character == '-' {
            capitalise = true;
        } else if capitalise {
            out.extend(character.to_uppercase());
            capitalise = false;
        } else {
            out.push(character);
        }
    }

    out
}

/// Every file this palette generates, as a repository-relative path and the
/// text that belongs in it.
///
/// One list, used by the generator and by the test that checks the checked-in
/// files still match. A second list would be a way for those two to disagree.
#[must_use]
pub fn generated() -> Vec<(&'static str, String)> {
    vec![
        ("apps/desktop/ui/tokens.css", css()),
        ("apps/android/app/src/main/res/values/colors.xml", android()),
        ("apps/ios/Sources/Ephemeral/Palette.swift", swift()),
    ]
}

/// Where the repository is, worked out from this crate's own location rather
/// than from where somebody happened to run the command.
#[must_use]
pub fn repository_root() -> std::path::PathBuf {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    here.canonicalize().unwrap_or(here)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_schemes_carry_the_same_names() {
        let dark: Vec<&str> = DARK.colours.iter().map(|colour| colour.name).collect();
        let light: Vec<&str> = LIGHT.colours.iter().map(|colour| colour.name).collect();

        assert_eq!(
            dark, light,
            "anything drawn in one scheme has to be drawable in the other"
        );
    }

    #[test]
    fn every_colour_is_a_colour() {
        for scheme in SCHEMES {
            for colour in scheme.colours {
                Rgb::parse(colour.hex)
                    .unwrap_or_else(|error| panic!("{}/{}: {error}", scheme.name, colour.name));
                assert!(
                    !colour.what.is_empty(),
                    "{}/{} says what it is for",
                    scheme.name,
                    colour.name
                );
            }
        }
    }

    /// The ratio the specification's own worked example gives.
    #[test]
    fn contrast_matches_the_specification() {
        let black = Rgb::parse("#000000").expect("black");
        let white = Rgb::parse("#ffffff").expect("white");

        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.001);
    }
}
