//! Gate: nothing in this crate calls `Ui::set_clip_rect` except the one helper that narrows.
//!
//! # Why a test and not a convention
//!
//! `Ui::set_clip_rect` **replaces** the clip rectangle; it does not intersect it with what the
//! parent had. A child `Ui` that clips itself to a rectangle of its own making therefore does not
//! restrict itself — it *frees* itself, and may paint anywhere on the layer. The tab bar did
//! exactly that, and a leaf too short for a whole tab bar painted the difference through the
//! enclosing window's border and out onto the desktop (see FINDINGS).
//!
//! The two other calls in the crate were correct at the time — each happened to hand over a
//! rectangle smaller than the one it already had — and that is precisely the problem: their
//! correctness was an argument about the surrounding code, one that has to be made again every
//! time either side of it is edited, by someone who has to know the method replaces in the first
//! place. `utils::clip_to` makes the narrowing unconditional, and this gate makes going around it
//! visible.
//!
//! # What is checked
//!
//! Every `.rs` file under `src/`, comments stripped, must not mention `set_clip_rect` — except
//! `src/utils.rs`, where the helper is, and where exactly one mention is expected. A doc comment
//! may name the method freely; naming it is how the rule is explained.
//!
//! The scan counts what it looked at and asserts the count, for the reason `core_is_egui_free`
//! gives: a scanner that silently finds nothing because it scanned nothing reads exactly like a
//! clean bill of health.

use std::path::{Path, PathBuf};

/// Lower bound on the files the gate must see. `src/` is around 30 files today; if a refactor
/// legitimately shrinks it below this, lower the bound *deliberately*.
const MIN_FILES_SCANNED: usize = 25;

/// Lower bound on non-comment code lines scanned, for the same reason. `src/` is some 7000 such
/// lines today.
const MIN_CODE_LINES_SCANNED: usize = 5000;

/// The one file allowed to name the method in code, and the one call it is allowed to make.
const HELPER: &str = "utils.rs";

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
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

/// The file with its comments blanked out, line by line.
///
/// Deliberately crude — line comments and whole-line block comments, no string-literal awareness —
/// because it is a *reader* of the crate's own source, and the crate's own source has no
/// `set_clip_rect` inside a string. A stricter parser here would be more machinery guarding the
/// same one word.
fn code_lines(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in source.lines() {
        let mut line = line.trim().to_owned();
        if in_block {
            match line.find("*/") {
                Some(end) => {
                    line = line[end + 2..].to_owned();
                    in_block = false;
                }
                None => continue,
            }
        }
        if let Some(start) = line.find("/*") {
            in_block = !line[start..].contains("*/");
            line.truncate(start);
        }
        if let Some(start) = line.find("//") {
            line.truncate(start);
        }
        let line = line.trim().to_owned();
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}

#[test]
fn only_the_helper_sets_a_clip_rect() {
    let mut files = 0;
    let mut code = 0;
    let mut helper_calls = 0;
    let mut violations = Vec::new();

    for file in rs_files(&src_dir()) {
        files += 1;
        let is_helper = file.file_name().is_some_and(|name| name == HELPER);
        let source = std::fs::read_to_string(&file).unwrap();
        for (number, line) in code_lines(&source).into_iter().enumerate() {
            code += 1;
            if !line.contains("set_clip_rect") {
                continue;
            }
            if is_helper {
                helper_calls += 1;
            } else {
                violations.push(format!(
                    "{}:{}: {line}",
                    file.strip_prefix(src_dir().parent().unwrap())
                        .unwrap()
                        .display(),
                    number + 1
                ));
            }
        }
    }

    assert!(
        files >= MIN_FILES_SCANNED,
        "scanned only {files} files, so finding no violation means nothing"
    );
    assert!(
        code >= MIN_CODE_LINES_SCANNED,
        "scanned only {code} lines of code, so finding no violation means nothing"
    );
    assert_eq!(
        helper_calls, 1,
        "`{HELPER}` should hold exactly one `set_clip_rect` — the one inside `clip_to`. Found \
         {helper_calls}: either the helper grew a second way to clip, or it lost the first."
    );
    assert!(
        violations.is_empty(),
        "`Ui::set_clip_rect` replaces the clip rectangle rather than narrowing it — use \
         `utils::clip_to`:\n{}",
        violations.join("\n")
    );
}
