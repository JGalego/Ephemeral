//! The reference WebAssembly application.
//!
//! What a generated application looks like when it runs on a phone. Not a test
//! fixture: it is a real program, written the way a person would write one,
//! compiled to `wasm32-wasip1`, and run through exactly the sandbox any other
//! application gets.
//!
//! It exists because everything else that exercises the WebAssembly runtime
//! assembles a module out of WebAssembly text. That proves the runtime holds;
//! it does not prove somebody could *write* something for it. This is the
//! difference between "the sandbox works" and "there is an application in it".
//!
//! ## What it demonstrates
//!
//! **Tier two** — it declares what it takes, and a client draws a form from
//! that declaration rather than asking somebody to type a command line. The
//! declaration lives in the manifest beside it; the arguments here are what
//! that form composes.
//!
//! **Tier one** — with `--format html` it writes a page instead of a line, and
//! the host renders it. A WebAssembly application has no socket and cannot be
//! a server, which is exactly why showing somebody a user interface costs no
//! network permission at all.
//!
//! ## What it cannot do, and does not try to
//!
//! It has no dependencies, opens no socket, and reads only the file it was
//! given. If it asked for anything else the module would not start — not
//! because this code is careful, but because there is nothing for the request
//! to bind to.

use std::io::Read as _;

/// How the answer is written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    /// One line, for a terminal.
    Line,
    /// A page, for a window or a phone.
    Page,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match run(&arguments) {
        Ok(said) => print!("{said}"),
        Err(said) => {
            // Standard error and a non-zero exit, because a program that failed
            // saying so on standard output is a program whose failure looks
            // like an answer.
            eprintln!("{said}");
            std::process::exit(1);
        }
    }
}

/// Reads the file it was given and describes what is in it.
fn run(arguments: &[String]) -> Result<String, String> {
    let asked = Asked::from(arguments)?;
    let text = read(&asked.file)?;
    let counted = Counted::of(&text, asked.headers);

    Ok(match asked.format {
        Format::Line => counted.line(&asked.file),
        Format::Page => counted.page(&asked.file),
    })
}

/// What somebody filled in.
struct Asked {
    file: String,
    headers: bool,
    format: Format,
}

impl Asked {
    /// Reads the argument vector the domain composed from a form.
    ///
    /// Strict about what it does not recognise. A generated application that
    /// silently ignored an argument would be one that quietly did something
    /// other than what the form said.
    fn from(arguments: &[String]) -> Result<Self, String> {
        let mut file = None;
        let mut headers = true;
        let mut format = Format::Line;

        let mut rest = arguments.iter();
        while let Some(argument) = rest.next() {
            match argument.as_str() {
                "--file" => {
                    file = Some(
                        rest.next()
                            .ok_or("--file needs a path after it")?
                            .to_owned(),
                    );
                }
                "--no-headers" => headers = false,
                "--format" => {
                    format = match rest.next().map(String::as_str) {
                        Some("html") => Format::Page,
                        Some("text") | None => Format::Line,
                        Some(other) => return Err(format!("{other} is not a format I know")),
                    };
                }
                other => return Err(format!("I do not know what {other} means")),
            }
        }

        Ok(Self {
            file: file.ok_or("no file was given, so there is nothing to count")?,
            headers,
            format,
        })
    }
}

/// Opens a file and says something useful when it cannot.
///
/// The message names the sandbox, because that is the likeliest reason and the
/// one somebody can do something about. "No such file or directory" for a file
/// somebody can see in their own folder is a confusing way to learn that an
/// application was not granted it.
fn read(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        format!(
            "{path} could not be opened: {error}. \
             If it is there, this application may not have been allowed to read it."
        )
    })?;

    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("{path} could not be read: {error}"))?;

    Ok(text)
}

/// What was in the file.
struct Counted {
    rows: usize,
    columns: usize,
    empty: usize,
}

impl Counted {
    /// Counts the rows and works out how wide they are.
    fn of(text: &str, headers: bool) -> Self {
        // Blank lines are not rows. A file ending in a newline would otherwise
        // report one row more than anybody can see in it, which is the sort of
        // off-by-one that makes somebody stop trusting a tool.
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();

        let columns = lines.first().map_or(0, |line| line.split(',').count());
        let rows = if headers {
            lines.len().saturating_sub(1)
        } else {
            lines.len()
        };
        let empty = text.lines().filter(|line| line.trim().is_empty()).count();

        Self {
            rows,
            columns,
            empty,
        }
    }

    /// One line, for a terminal.
    fn line(&self, file: &str) -> String {
        format!(
            "{file}: {} {}, {} {}{}\n",
            self.rows,
            plural(self.rows, "row", "rows"),
            self.columns,
            plural(self.columns, "column", "columns"),
            if self.empty == 0 {
                String::new()
            } else {
                format!(
                    ", and {} blank {} ignored",
                    self.empty,
                    plural(self.empty, "line", "lines")
                )
            }
        )
    }

    /// A page, for a window or a phone.
    ///
    /// Plain and self-contained: no script, no image, no font, nothing fetched
    /// from anywhere. A host renders this with scripts off and every
    /// subresource blocked, so a page that needed any of those would render as
    /// a broken version of itself.
    fn page(&self, file: &str) -> String {
        format!(
            "<!DOCTYPE html>\n\
             <meta charset=\"utf-8\">\n\
             <title>{name}</title>\n\
             <style>\n\
             body {{ font: 16px/1.5 system-ui, sans-serif; margin: 2rem; color: #131728; }}\n\
             h1 {{ font-size: 1.25rem; margin: 0 0 1rem; }}\n\
             dl {{ display: grid; grid-template-columns: auto 1fr; gap: 0.4rem 1.5rem; margin: 0; }}\n\
             dt {{ color: #525a74; }}\n\
             dd {{ margin: 0; font-variant-numeric: tabular-nums; font-weight: 600; }}\n\
             </style>\n\
             <h1>{name}</h1>\n\
             <dl>\n\
             <dt>Rows</dt><dd>{rows}</dd>\n\
             <dt>Columns</dt><dd>{columns}</dd>\n\
             <dt>Blank lines ignored</dt><dd>{empty}</dd>\n\
             </dl>\n",
            name = escape(file),
            rows = self.rows,
            columns = self.columns,
            empty = self.empty,
        )
    }
}

/// The singular or the plural, because "1 rows" reads like a bug.
fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// Escapes text for a page.
///
/// The file name comes from a form somebody filled in, so it is the one thing
/// on the page this application did not write. The host renders with scripts
/// disabled, which means a `<script>` here would not run — but a page whose
/// heading can be closed early and rewritten is still a page that lies about
/// what it counted.
fn escape(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            _ => character.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_row_is_not_counted_as_a_row() {
        let counted = Counted::of("name,size\na,1\nb,2\n", true);
        assert_eq!(counted.rows, 2);
        assert_eq!(counted.columns, 2);
    }

    #[test]
    fn a_file_without_headers_counts_every_line() {
        let counted = Counted::of("a,1\nb,2\n", false);
        assert_eq!(counted.rows, 2);
    }

    /// A trailing newline is not a row. Reporting one more row than somebody
    /// can see is the sort of off-by-one that makes them stop trusting a tool.
    #[test]
    fn a_trailing_newline_is_not_a_row() {
        assert_eq!(Counted::of("name\na\n", true).rows, 1);
        assert_eq!(Counted::of("name\na", true).rows, 1);
    }

    #[test]
    fn an_empty_file_is_nothing_rather_than_a_failure() {
        let counted = Counted::of("", true);
        assert_eq!(counted.rows, 0);
        assert_eq!(counted.columns, 0);
    }

    #[test]
    fn a_flag_that_is_off_is_not_passed_and_is_not_assumed() {
        let asked = Asked::from(&["--file".to_owned(), "a.csv".to_owned()]).unwrap();
        assert!(asked.headers, "the default is what the manifest declares");

        let asked = Asked::from(&[
            "--file".to_owned(),
            "a.csv".to_owned(),
            "--no-headers".to_owned(),
        ])
        .unwrap();
        assert!(!asked.headers);
    }

    /// An argument it does not understand is a refusal, not something to skip.
    /// Quietly ignoring one would mean doing something other than what the form
    /// said.
    #[test]
    fn an_argument_it_does_not_know_is_refused() {
        assert!(Asked::from(&["--wat".to_owned()]).is_err());
        assert!(Asked::from(&[]).is_err(), "and no file is nothing to count");
    }

    /// The file name is the one thing on the page this application did not
    /// write, and it comes from a form.
    #[test]
    fn a_name_from_a_form_cannot_rewrite_the_page() {
        let page = Counted::of("a\n", false).page("<script>alert(1)</script>");
        assert!(!page.contains("<script>"), "{page}");
        assert!(page.contains("&lt;script&gt;"));
    }

    #[test]
    fn one_row_is_a_row_rather_than_rows() {
        assert!(
            Counted::of("name\na\n", true)
                .line("f.csv")
                .contains("1 row,")
        );
        assert!(
            Counted::of("name\na\nb\n", true)
                .line("f.csv")
                .contains("2 rows,")
        );
    }
}
