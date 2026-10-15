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

/// The line numbers (from 1) of every `#[doc(hidden)]` item in `text` that is not gated: a test hook that production code could
/// call. An item is gated when an attribute `#[cfg(test)]` or `#[cfg(feature = "fakes")]` stands among the attributes and doc
/// comments right above it or between `#[doc(hidden)]` and the item.
fn ungated_doc_hidden(text: &str) -> Vec<usize> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let part_of_the_head = |l: &str| l.starts_with("#[") || l.starts_with("///");
    let mut found = Vec::new();
    for (at, line) in lines.iter().enumerate() {
        if *line != "#[doc(hidden)]" {
            continue;
        }
        let mut head = vec![*line];
        head.extend(
            lines[..at]
                .iter()
                .rev()
                .take_while(|l| part_of_the_head(l))
                .copied(),
        );
        head.extend(
            lines[at + 1..]
                .iter()
                .take_while(|l| part_of_the_head(l))
                .copied(),
        );
        let gated = head
            .iter()
            .any(|l| *l == "#[cfg(test)]" || *l == "#[cfg(feature = \"fakes\")]");
        if !gated {
            found.push(at + 1);
        }
    }
    found
}

/// A `#[doc(hidden)]` item is a test hook that `pub` makes part of the crate's API. It must not be reachable from the hub, so it
/// exists only under `cfg(test)` (this crate's unit tests) or the `fakes` feature (this crate's integration tests, which turn it on
/// through the crate's own dev-dependency, and nothing in the hub's build does).
#[test]
fn a_hidden_test_hook_exists_only_in_test_builds() {
    for (path, text) in sources() {
        assert_eq!(
            ungated_doc_hidden(&text),
            Vec::<usize>::new(),
            "{} has a #[doc(hidden)] item that a production build can reach",
            path.display()
        );
    }
}

/// Positive controls: the check fails for the hooks as they were, and passes for the gated ones.
#[test]
fn the_check_for_ungated_hooks_finds_what_it_should() {
    // As `McpState::forget_session_owners` and `Replay::run_pending_tasks` were before they were gated.
    let ungated = "    /// Forget who opened which session.\n    #[doc(hidden)]\n    pub fn forget_session_owners(&self) {\n    }\n\n    /// Run the cache's pending maintenance.\n    #[doc(hidden)]\n    #[must_use]\n    pub fn weighted_size(&self) -> u64 {\n        0\n    }\n";
    assert_eq!(ungated_doc_hidden(ungated), [2, 7]);
    for gated in [
        "    #[cfg(test)]\n    #[doc(hidden)]\n    pub fn a(&self) {}\n",
        "    #[cfg(feature = \"fakes\")]\n    #[doc(hidden)]\n    pub fn a(&self) {}\n",
        "    /// Doc.\n    #[cfg(feature = \"fakes\")]\n    /// More doc.\n    #[doc(hidden)]\n    #[must_use]\n    pub fn a(&self) {}\n",
        "    /// Doc.\n    #[doc(hidden)]\n    #[cfg(test)]\n    pub fn a(&self) {}\n",
    ] {
        assert_eq!(ungated_doc_hidden(gated), Vec::<usize>::new(), "{gated}");
    }
    // A gate on another item above is not a gate for this one.
    let other = "#[cfg(test)]\nfn a() {}\n\n#[doc(hidden)]\npub fn b() {}\n";
    assert_eq!(ungated_doc_hidden(other), [4]);
    // The wrong feature is not a gate.
    let wrong = "#[cfg(feature = \"postgres\")]\n#[doc(hidden)]\npub fn b() {}\n";
    assert_eq!(ungated_doc_hidden(wrong), [2]);
}
