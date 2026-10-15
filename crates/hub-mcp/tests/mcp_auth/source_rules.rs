//! Rules 1, 2, 4 and 5 of AGENTS.md and S14 ("no tool executes a shell"), checked on the text of `src/`. Clippy and the compiler
//! hold the rest (`unwrap` and friends are denied in the workspace lints).

use std::fs;
use std::path::{Path, PathBuf};

fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = fs::read_to_string(&path).unwrap();
                out.push((path, text));
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(out.len() >= 10, "found only {} source files", out.len());
    out
}

/// Code lines only: a comment may mention what it forbids.
fn code(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim_start().starts_with("//"))
}

fn forbid(patterns: &[&str], why: &str) {
    for (path, text) in sources() {
        for line in code(&text) {
            for pattern in patterns {
                assert!(
                    !line.contains(pattern),
                    "{} uses `{pattern}` ({why}): {line}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn forbid_unsafe_present() {
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    assert!(lib.contains("#![forbid(unsafe_code)]"));
    forbid(&["unsafe {", "unsafe fn", "unsafe impl"], "rule 1");
}

#[test]
fn no_shell_or_process_in_src() {
    forbid(
        &[
            "std::process",
            "tokio::process",
            "Command::new",
            "libc::system",
            "/bin/sh",
        ],
        "S14: no tool executes a shell",
    );
}

#[test]
fn no_unbounded_channel_in_src() {
    forbid(
        &[
            "unbounded_channel",
            "mpsc::unbounded",
            "UnboundedSender",
            "unbounded(",
        ],
        "rule 5",
    );
}

#[test]
fn no_blocking_fs_in_src() {
    forbid(
        &[
            "std::fs",
            "std::thread::sleep",
            "std::net::TcpStream",
            "File::open",
            "read_to_string(",
        ],
        "rule 4: blocking calls in async code",
    );
}

#[test]
fn nothing_secret_is_formatted_into_a_message() {
    // A cheap check of rule 6 and S10: no log macro or error message in `src/` formats a token, a key or a hash.
    for (path, text) in sources() {
        for line in code(&text) {
            let logs = ["tracing::", "info!(", "warn!(", "error!(", "debug!(", "trace!("]
                .iter()
                .any(|m| line.contains(m));
            if logs {
                for word in ["token.expose", "expose()", "hmac", "bearer", "authorization"] {
                    assert!(
                        !line.to_ascii_lowercase().contains(word),
                        "{} logs `{word}`: {line}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn redacted_text_is_made_only_in_redact_rs() {
    // The type that stands for scanned text has a private constructor. Nothing else in `src/` builds one by hand.
    for (path, text) in sources() {
        if path.ends_with("output/redact.rs") {
            continue;
        }
        for line in code(&text) {
            assert!(
                !line.contains("RedactedText(")
                    || line.contains("-> RedactedText(")
                    || line.contains("RedactedText(Option"),
                "{} builds a RedactedText: {line}",
                path.display()
            );
        }
    }
}

#[test]
fn every_text_field_of_an_output_type_is_redacted_text() {
    // A field called `text` or `snippet` in an output type is a RedactedText, never a String.
    let dto = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/dto.rs")).unwrap();
    for line in code(&dto) {
        let t = line.trim_start();
        for field in ["pub text:", "pub snippet:"] {
            if let Some(rest) = t.strip_prefix(field) {
                assert!(rest.trim_start().starts_with("RedactedText"), "{line}");
            }
        }
    }
}
