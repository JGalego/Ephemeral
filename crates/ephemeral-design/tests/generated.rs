#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The checked-in palette files still match the palette.
//!
//! They are generated and committed rather than built, because the window has
//! no build step and should not acquire one to draw a list. The cost of that
//! choice is that a file can be edited by hand and nothing notices — which is
//! exactly how one client ends up with its own quietly different red.
//!
//! So this is the notice. It fails with the command that fixes it.

#[test]
fn the_generated_files_are_current() {
    let root = ephemeral_design::repository_root();
    let mut stale = Vec::new();

    for (path, expected) in ephemeral_design::generated() {
        let at = root.join(path);
        let found = std::fs::read_to_string(&at).unwrap_or_default();

        // Compared without line endings. Git on Windows checks text out as
        // CRLF, and this test failed on that runner for a difference nobody
        // wrote and nobody can see — while what it is actually asserting is
        // that the colours are the same colours. `.gitattributes` pins these
        // two files to LF as well, so both halves of the problem are fixed.
        if found.replace("\r\n", "\n") != expected {
            stale.push(path);
        }
    }

    assert!(
        stale.is_empty(),
        "the palette changed but these were not regenerated:\n  {}\n\nRun: cargo run -p ephemeral-design",
        stale.join("\n  ")
    );
}

/// Neither generated file may be empty or missing: an empty stylesheet is a
/// window with no colours at all, and the test above would pass happily if the
/// generator started producing nothing.
#[test]
fn the_generated_files_contain_a_palette() {
    for (path, contents) in ephemeral_design::generated() {
        assert!(
            contents.len() > 500,
            "{path} came out at {} bytes, which is not a palette",
            contents.len()
        );
        assert!(
            contents.contains("#0b0e17"),
            "{path} does not contain the ground colour"
        );
    }
}

/// The window the operating system draws is not the page inside it.
///
/// Tauri paints its own background before a single line of CSS is parsed, and
/// a window that is white for that fraction of a second is a window that
/// flashes — which is most of what somebody hates about a light application
/// they open twenty times a day. The colour lives in `tauri.conf.json` because
/// that is where Tauri reads it, and it is checked here because two places
/// holding the same colour is one place being wrong.
#[test]
fn the_window_the_system_draws_is_the_same_colour_as_the_page() {
    let config = std::fs::read_to_string(
        ephemeral_design::repository_root().join("apps/desktop/src-tauri/tauri.conf.json"),
    )
    .expect("the window is configured");

    let ground = ephemeral_design::DARK
        .hex("ground")
        .expect("the ground colour");

    assert!(
        config.contains(&format!("\"backgroundColor\": \"{ground}\"")),
        "tauri.conf.json should paint the window {ground} before the page loads"
    );
}
