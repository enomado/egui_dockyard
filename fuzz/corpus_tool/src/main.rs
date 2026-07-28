//! Builds (and re-checks) the seed corpus for the `tree_persist` fuzz target out of real saved
//! layouts.
//!
//! # Why seed at all
//!
//! Left to itself, a fuzzer starting from nothing spends its first millions of executions
//! learning to produce a syntactically valid RON document, and never gets to the shapes that
//! matter: a deeply unbalanced tree, the pre-arena heap form with its `Empty` holes, a focus
//! route pointing into a subtree that a repair dropped. Those shapes exist already — in files
//! users saved — so the corpus is harvested rather than invented.
//!
//! # What it does
//!
//! Saved layouts of the application that vendors this fork wrap the dock state in a field named
//! `tab`, next to unrelated application state. This tool lifts that field out verbatim (byte for
//! byte — the point is to keep the original text, including fields the current format no longer
//! writes), checks that the reader both accepts it and returns a well-formed dock state, and
//! writes it to the corpus directory. Files that are already a bare dock state are taken whole.
//!
//! A source file that cannot be turned into a corpus entry is an error, not a skip: silently
//! seeding nothing looks exactly like seeding everything from the outside, and then the fuzzer
//! is starting from scratch while the report says "seeded with real layouts".
//!
//! ```text
//! cargo run --manifest-path fuzz/corpus_tool/Cargo.toml -- <layouts-dir> fuzz/corpus/tree_persist
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use egui_dock::DockState;

/// Tabs are opaque here for the same reason they are opaque in the fuzz target: the format
/// under test is the layout around them, not their payload.
type Tab = ron::Value;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, source, destination] = args.as_slice() else {
        eprintln!("usage: corpus_tool <layouts-dir> <corpus-dir>");
        return ExitCode::FAILURE;
    };
    let (source, destination) = (Path::new(source), Path::new(destination));

    let mut files: Vec<PathBuf> = match std::fs::read_dir(source) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file())
            .collect(),
        Err(error) => {
            eprintln!("cannot read {}: {error}", source.display());
            return ExitCode::FAILURE;
        }
    };
    // Directory order is not stable across machines; the corpus should be.
    files.sort();

    if let Err(error) = std::fs::create_dir_all(destination) {
        eprintln!("cannot create {}: {error}", destination.display());
        return ExitCode::FAILURE;
    }

    let mut written = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                failures.push(format!("{name}: unreadable ({error})"));
                continue;
            }
        };

        let dock_state = match extract_field(&text, "tab") {
            Some(extracted) => extracted,
            None => text.clone(),
        };
        let dock_state = scrub_tabs(&dock_state);

        // Both halves matter. Parsing proves the entry reaches the code under test at all;
        // validating proves the entry is a *legal* starting point, so that a later fuzz
        // failure means "some mutation broke it", not "the seed was already broken".
        let state = match ron::from_str::<DockState<Tab>>(&dock_state) {
            Ok(state) => {
                if let Err(violations) = state.validate() {
                    failures.push(format!("{name}: parses but is not well-formed: {violations:?}"));
                    continue;
                }
                state
            }
            Err(error) => {
                failures.push(format!("{name}: not a dock state ({error})"));
                continue;
            }
        };

        let out = destination.join(sanitize(&name));
        if let Err(error) = std::fs::write(&out, dock_state.as_bytes()) {
            failures.push(format!("{name}: cannot write {}: {error}", out.display()));
            continue;
        }
        written += 1;

        // Every real file on disk is in the *old* form, so a corpus of them alone leaves the
        // current reader unseeded — the fuzzer would have to invent the new shape from
        // scratch (a first 60-second run over old-form seeds did not get near it). Writing
        // each layout back out through the current writer gives it a foothold in both forms.
        match ron::ser::to_string_pretty(&state, ron::ser::PrettyConfig::default()) {
            Ok(current_form) => {
                let out = destination.join(format!("{}.current", sanitize(&name)));
                if let Err(error) = std::fs::write(&out, current_form.as_bytes()) {
                    failures.push(format!("{name}: cannot write {}: {error}", out.display()));
                    continue;
                }
                written += 1;
            }
            Err(error) => {
                failures.push(format!("{name}: cannot be written back out ({error})"));
                continue;
            }
        }
    }

    println!(
        "{} file(s) read, {written} written to {}",
        files.len(),
        destination.display()
    );
    if !failures.is_empty() {
        eprintln!("{} file(s) rejected:", failures.len());
        for failure in &failures {
            eprintln!("  {failure}");
        }
        return ExitCode::FAILURE;
    }
    if written == 0 {
        eprintln!("nothing was written — an empty corpus is not a seeded corpus");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Replaces the contents of every `tabs: [...]` array with as many neutral strings as it had
/// elements.
///
/// Two reasons, and the first is the load-bearing one. A seed corpus is committed to a public
/// repository, and these files come from a real application: their tab payloads name its
/// internals. The shape is what this fuzzer is about — how leaves nest, which splits carry
/// which fractions, where the holes are — and the tab type is opaque to it (`ron::Value` in the
/// target, any `Tab: Deserialize` in the library), so the payloads can go without costing a
/// single branch of coverage. Second, they are bulky: dropping them makes the corpus small
/// enough that libFuzzer's mutations land on structure rather than on strings.
///
/// The count is preserved because it is structure: a leaf's active-tab index is read against
/// it, and an empty leaf is a different case from a leaf with one tab.
fn scrub_tabs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(offset) = rest.find("tabs:") {
        let (before, after) = rest.split_at(offset);
        out.push_str(before);

        let Some(open) = after.find('[') else {
            out.push_str(after);
            return out;
        };
        // Anything between `tabs:` and its `[` is whitespace; keep it as it is.
        out.push_str(&after[..=open]);

        let array = balanced_value(&after[open..]);
        let inner = &array[1..array.len() - 1];
        let count = top_level_elements(inner);
        let scrubbed: Vec<String> = (0..count).map(|index| format!("\"t{index}\"")).collect();
        out.push_str(&scrubbed.join(", "));
        out.push(']');

        rest = &after[open + array.len()..];
    }
    out.push_str(rest);
    out
}

/// Counts the elements of a bracket-free-at-depth-0 comma-separated list, ignoring commas
/// nested inside a value and a trailing comma.
fn top_level_elements(inner: &str) -> usize {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut elements = 0usize;
    let mut seen_content = false;

    for byte in inner.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                seen_content = true;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                seen_content = true;
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                elements += 1;
                seen_content = false;
            }
            byte if !byte.is_ascii_whitespace() => seen_content = true,
            _ => {}
        }
    }
    // A list without a trailing comma ends on content rather than on a separator.
    elements + usize::from(seen_content)
}

/// Lifts the value of a top-level `name: (...)` field out of a RON document, verbatim.
///
/// Scans for the field name at nesting depth 1 (the document's own outer parentheses being
/// depth 0→1) and then copies from its opening bracket to the matching one. Bracket counting
/// is string-aware, because a tab payload may well contain a `)` inside a string literal.
fn extract_field(text: &str, name: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];

        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            index += 1;
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {
                // A field of the document itself: `name` at depth 1, followed by a colon.
                if depth == 1 && text[index..].starts_with(name) {
                    let after_name = index + name.len();
                    let rest = text[after_name..].trim_start();
                    // Guard against matching a longer identifier that merely starts with
                    // `name` (`tabs:` is not `tab:`).
                    let boundary = !text[..index]
                        .chars()
                        .next_back()
                        .is_some_and(|char| char.is_alphanumeric() || char == '_');
                    if boundary && rest.starts_with(':') {
                        let value_start = after_name + text[after_name..].find(':')? + 1;
                        return Some(balanced_value(&text[value_start..]).trim().to_string());
                    }
                }
            }
        }
        index += 1;
    }

    None
}

/// Takes the leading value of `text`: everything up to the point where the brackets opened by
/// it are closed again.
fn balanced_value(text: &str) -> &str {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut started = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'(' | b'[' | b'{' => {
                depth += 1;
                started = true;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                if started && depth == 0 {
                    return &text[..=index];
                }
            }
            _ => {}
        }
    }
    text
}

/// libFuzzer passes corpus entries around by file name, so keep them boring.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || char == '.' || char == '-' {
                char
            } else {
                '_'
            }
        })
        .collect()
}
