//! Gate: the model in `src/core/` must not depend on `egui`.
//!
//! # Why a test and not a convention
//!
//! "We agreed not to import egui in the core" rots silently — one `use egui::Rect` in a
//! hurry and the property tests, the fuzz target and the deterministic simulator all
//! quietly acquire a UI dependency again. This is the same class of drift that
//! `tools/vendor_vs_fork.sh` catches between vendor and fork, so it gets the same
//! treatment: a check that fails loudly.
//!
//! # What is checked
//!
//! Every `.rs` file under `src/core/`, with comments stripped, must not mention `egui`.
//! Doc comments are exempt on purpose: they carry doctests, which are compiled as
//! *external* users of the crate and may legitimately reach for `egui` — a doctest is not
//! part of the core's dependency graph.
//!
//! The check counts what it looked at and asserts that count. A scanner that silently
//! finds nothing because it scanned nothing is the failure mode this guards against.

use std::path::{Path, PathBuf};

/// Lower bound on the files the gate must see. The core is 17 files today; if a refactor
/// legitimately shrinks it below this, lower the bound *deliberately* — do not delete the
/// assertion, or "0 violations" starts meaning "0 files scanned".
const MIN_FILES_SCANNED: usize = 15;

/// Lower bound on non-comment code lines scanned, for the same reason: a comment-stripper
/// bug that blanks every file would otherwise read as a clean bill of health. The core is
/// around 2500 such lines today.
const MIN_CODE_LINES_SCANNED: usize = 2000;

fn core_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core")
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(rs_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Strip comments so that only lines the compiler treats as code remain.
///
/// Deliberately simple: line comments (`//`, `///`, `//!`) are dropped whole, block
/// comments are tracked with a depth counter (Rust nests them). String literals
/// containing `//` would be mangled by this — acceptable, because the result is only ever
/// searched for the word `egui`, never re-parsed.
fn strip_comments(source: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut block_depth = 0usize;

    for (number, raw) in source.lines().enumerate() {
        let mut line = String::new();
        let bytes: Vec<char> = raw.chars().collect();
        let mut i = 0;
        while i < bytes.len() {
            let two: String = bytes[i..(i + 2).min(bytes.len())].iter().collect();
            if block_depth > 0 {
                if two == "*/" {
                    block_depth -= 1;
                    i += 2;
                } else if two == "/*" {
                    block_depth += 1;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if two == "/*" {
                block_depth += 1;
                i += 2;
                continue;
            }
            if two == "//" {
                break;
            }
            line.push(bytes[i]);
            i += 1;
        }
        let line = line.trim().to_string();
        if !line.is_empty() {
            out.push((number + 1, line));
        }
    }
    out
}

#[test]
fn core_does_not_mention_egui() {
    let dir = core_dir();
    let files = rs_files(&dir);
    let mut code_lines = 0usize;
    let mut violations: Vec<String> = Vec::new();

    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        for (number, line) in strip_comments(&source) {
            code_lines += 1;
            if line.contains("egui") {
                let relative = file.strip_prefix(&dir).unwrap().display();
                violations.push(format!("src/core/{relative}:{number}: {line}"));
            }
        }
    }

    // Coverage first: an empty scan must never pass as "clean".
    assert!(
        files.len() >= MIN_FILES_SCANNED,
        "the gate scanned only {} files under {} — it is not looking where it should",
        files.len(),
        dir.display()
    );
    assert!(
        code_lines >= MIN_CODE_LINES_SCANNED,
        "the gate scanned only {code_lines} code lines — comment stripping is eating the source"
    );

    assert!(
        violations.is_empty(),
        "the core must stay egui-free, but {} line(s) mention it:\n{}\n\
         Geometry that the renderer derives belongs in `src/layout/`; geometry that is \
         genuine state belongs in `core::geom` (egui-free, wire-compatible).",
        violations.len(),
        violations.join("\n")
    );
}

/// The gate itself must bite. A detector nobody has ever seen fail is indistinguishable
/// from a detector that cannot fail — so feed it the exact shapes it is meant to catch,
/// and the shapes it must not.
#[test]
fn the_gate_can_actually_fail() {
    let offending = "use egui::Rect;\nfn f() {}\n";
    assert!(
        strip_comments(offending)
            .iter()
            .any(|(_, line)| line.contains("egui")),
        "a plain `use egui::Rect;` must be caught"
    );

    let doc_only = "/// ```rust\n/// # use egui::Rect;\n/// ```\npub fn f() {}\n";
    assert!(
        strip_comments(doc_only)
            .iter()
            .all(|(_, line)| !line.contains("egui")),
        "doctests are external users of the crate and must not trip the gate"
    );

    let block_comment = "/* egui::Rect\n   still a comment */\npub fn f() {}\n";
    assert!(
        strip_comments(block_comment)
            .iter()
            .all(|(_, line)| !line.contains("egui")),
        "block comments must be stripped, including their continuation lines"
    );

    let trailing = "pub fn f() {} // egui mentioned in a trailing comment\n";
    assert!(
        strip_comments(trailing)
            .iter()
            .all(|(_, line)| !line.contains("egui")),
        "a trailing comment must not trip the gate, but the code before it must survive"
    );
    assert_eq!(
        strip_comments(trailing).len(),
        1,
        "the code before a trailing comment must still be scanned"
    );
}
