//! Writes the palette out for every platform that draws it.
//!
//! `cargo run -p ephemeral-design`. The files it writes are checked in, and a
//! test asserts they still match what this would produce — so the generator is
//! not something anybody has to remember to run, only something they have to
//! run before pushing what they changed.

fn main() -> Result<(), std::io::Error> {
    let root = ephemeral_design::repository_root();

    for (path, contents) in ephemeral_design::generated() {
        let at = root.join(path);
        let changed = std::fs::read_to_string(&at).ok().as_deref() != Some(contents.as_str());

        std::fs::write(&at, &contents)?;
        println!(
            "{} {}",
            if changed { "wrote" } else { "unchanged" },
            at.display()
        );
    }

    Ok(())
}
