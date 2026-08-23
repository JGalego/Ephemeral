#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Every pairing Ephemeral draws, checked against WCAG 2.1 in both schemes.
//!
//! Risk is carried by colour in the terminal, the window and the phone. A risk
//! level somebody cannot read against the ground it sits on is a permission
//! prompt that quietly does not work — and the person it fails is the one least
//! able to say so. This is the check that keeps "accessible" from being a word
//! in a design document.
//!
//! It fails loudly and specifically: which scheme, which pairing, what it
//! reached and what it needed.

use ephemeral_design::{PAIRINGS, Rgb, SCHEMES, contrast};

#[test]
fn every_pairing_can_be_read_in_every_scheme() {
    let mut failures = Vec::new();

    for scheme in SCHEMES {
        for pairing in PAIRINGS {
            let fore =
                Rgb::parse(scheme.hex(pairing.fore).expect("a named colour")).expect("a colour");
            let back =
                Rgb::parse(scheme.hex(pairing.back).expect("a named colour")).expect("a colour");
            let ratio = contrast(fore, back);

            if ratio < pairing.least {
                failures.push(format!(
                    "{}: {} on {} is {ratio:.2}:1, needs {:.1}:1 — {}",
                    scheme.name, pairing.fore, pairing.back, pairing.least, pairing.what
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} pairing(s) cannot be read:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The four risk levels have to be four *different* colours, not four names for
/// the same one. A palette where medium and high are two shades apart is a
/// palette that tells somebody nothing at the moment they most need telling.
#[test]
fn the_risk_levels_are_visibly_different_from_each_other() {
    let levels = ["low", "medium", "high", "critical"];

    for scheme in SCHEMES {
        for (index, first) in levels.iter().enumerate() {
            for second in &levels[index + 1..] {
                let a = Rgb::parse(scheme.hex(first).expect("a named colour")).expect("a colour");
                let b = Rgb::parse(scheme.hex(second).expect("a named colour")).expect("a colour");

                assert_ne!(
                    a, b,
                    "{}: {first} and {second} are the same colour",
                    scheme.name
                );
            }
        }
    }
}

/// The contract has to cover every risk on every ground it is drawn on.
///
/// A pairing that is checked is a pairing somebody thought to list. This is the
/// check on the list itself: all four levels appear on the page, on a card, and
/// on their own tinted ground, in that order of how often they are drawn. A
/// palette can only be as accessible as its inventory is complete.
///
/// Colour is never the only carrier either. `ephemeral-cli` prints the level as
/// a word beside every permission, the window renders `risk-<level>` on an
/// element whose text says what it is, and both phones write the level next to
/// the consequence. Those are checked by their own suites; what belongs here is
/// that the palette is not asked to do the whole job alone.
#[test]
fn every_risk_is_checked_on_every_ground_it_is_drawn_on() {
    for level in ["low", "medium", "high", "critical"] {
        for ground in ["ground", "surface", &format!("{level}-soft")] {
            assert!(
                PAIRINGS
                    .iter()
                    .any(|pairing| pairing.fore == level && pairing.back == ground),
                "nothing checks {level} on {ground}"
            );
        }
    }
}

/// Printing the whole table is worth more than a pass line: somebody adjusting
/// a colour wants to see how much room they have, not only that they have some.
#[test]
fn the_whole_table_is_printed() {
    for scheme in SCHEMES {
        println!("\n{}", scheme.name);
        for pairing in PAIRINGS {
            let fore =
                Rgb::parse(scheme.hex(pairing.fore).expect("a named colour")).expect("a colour");
            let back =
                Rgb::parse(scheme.hex(pairing.back).expect("a named colour")).expect("a colour");

            println!(
                "  {:>7.2}:1  (needs {:>3.1})  {} on {}",
                contrast(fore, back),
                pairing.least,
                pairing.fore,
                pairing.back
            );
        }
    }
}
