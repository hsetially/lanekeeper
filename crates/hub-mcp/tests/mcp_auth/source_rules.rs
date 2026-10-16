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

/// The lines of production code (before a `#[cfg(test)] mod tests`, and not comments) that build a scan configuration of their own.
fn default_redactors(text: &str) -> usize {
    let production = text.split("#[cfg(test)]\nmod tests").next().unwrap_or(text);
    code(production).filter(|l| l.contains("Redactor::new()")).count()
}

/// Round 2, review item 13: the question for a pull request built a `Redactor::new()` of its own, so tuning the scan of the host (Q20)
/// would not have reached it. The host's `Redactor` is the only one the server builds, and the gate gives it to the message. (That
/// the message uses what it is given is `confirm::message::tests::the_description_is_scanned_with_the_redactor_it_is_given`.)
#[test]
fn the_host_builds_the_only_redactor_and_the_gate_gives_it_to_the_message() {
    let owners: Vec<PathBuf> = sources()
        .into_iter()
        .filter(|(_, text)| default_redactors(text) > 0)
        .map(|(path, _)| path)
        .collect();
    assert_eq!(owners.len(), 1, "a second scan configuration: {owners:?}");
    assert!(owners[0].ends_with("server/mod.rs"), "{owners:?}");
    let gate = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/confirm/gate.rs")).unwrap();
    let flat: String = gate.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("message::raise_pr(request, approval, link, &host.redactor)"),
        "the gate does not hand the host's redactor to the question"
    );
}

#[test]
fn the_check_for_a_second_redactor_finds_what_it_should() {
    assert_eq!(default_redactors("fn host() -> Redactor { Redactor::new() }"), 1);
    assert_eq!(
        default_redactors("let a = Redactor::new();\nlet b = Redactor::new();"),
        2
    );
    assert_eq!(
        default_redactors("// a comment about Redactor::new()\nfn f() {}"),
        0
    );
    assert_eq!(
        default_redactors("fn f() {}\n#[cfg(test)]\nmod tests {\n    fn g() { Redactor::new(); }\n}"),
        0,
        "a test may build its own"
    );
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
