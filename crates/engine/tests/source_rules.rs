//! T1 (S11, S21, acceptance 3): rules about the engine's source that every later task must keep. They are scans of
//! `src/`, with comments and literals blanked out so that a doc comment or a message cannot trip them (or hide a
//! violation).
//!
//! - no unsafe code;
//! - no `HashMap` or `HashSet`: their iteration order is random, and the engine's output must be byte-identical;
//! - no clock, file, network, process or environment access, and no async code or tokio: the engine is pure, and the
//!   caller decides when and where it runs;
//! - no raw string or `Path` parameters named like paths on public functions: a path is an `NfsPath`, a `LogicalFile`
//!   or another typed value, never text that nobody has checked (S11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every file under `src/`, as (path relative to the crate, code with comments and literals blanked).
fn sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_sources(&crate_dir().join("src"), &mut files);
    assert!(!files.is_empty(), "no sources found under src/");
    files
        .into_iter()
        .map(|f| {
            let text = fs::read_to_string(&f).unwrap();
            let rel = f
                .strip_prefix(crate_dir())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, code_only(&text))
        })
        .collect()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `src` with comments, string literals and char literals replaced by spaces (newlines kept), so that what is left is
/// code and nothing else.
fn code_only(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0_usize;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if let Some(hashes) = raw_start(&b, i) {
            // r"..." and r#"..."# (and the b-prefixed forms, whose `b` was already copied).
            let open = i + 1 + hashes + 1;
            out.extend(std::iter::repeat_n(' ', open - i));
            i = open;
            while i < b.len() {
                if b[i] == '"' && (0..hashes).all(|k| b.get(i + 1 + k) == Some(&'#')) {
                    out.extend(std::iter::repeat_n(' ', 1 + hashes));
                    i += 1 + hashes;
                    break;
                }
                out.push(blank(b[i]));
                i += 1;
            }
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    out.push(' ');
                    out.push(blank(b.get(i + 1).copied().unwrap_or(' ')));
                    i += 2;
                } else if b[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{1F600}', '"') or a lifetime ('a).
            let literal_end = if next == Some('\\') {
                (i + 3..b.len().min(i + 14)).find(|&k| b[k] == '\'')
            } else if b.get(i + 2) == Some(&'\'') {
                Some(i + 2)
            } else {
                None
            };
            if let Some(end) = literal_end {
                out.extend(std::iter::repeat_n(' ', end + 1 - i));
                i = end + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// At an `r` that starts a raw string: the number of `#` between it and the opening quote.
fn raw_start(b: &[char], i: usize) -> Option<usize> {
    if b[i] != 'r' || (i > 0 && is_ident(b[i - 1]) && b[i - 1] != 'b') {
        return None;
    }
    let mut k = i + 1;
    while b.get(k) == Some(&'#') {
        k += 1;
    }
    (b.get(k) == Some(&'"')).then_some(k - i - 1)
}

/// True when `word` appears in `code` as a whole identifier.
fn contains_word(code: &str, word: &str) -> bool {
    code.match_indices(word).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + word.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The text between the `{` that follows `std::` and its matching `}`, for each grouped `std::{...}` import.
fn std_groups(code: &str) -> Vec<&str> {
    code.match_indices("std::{")
        .filter_map(|(at, m)| {
            let start = at + m.len();
            let mut depth = 1_usize;
            for (k, c) in code[start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&code[start..start + k]);
                        }
                    }
                    _ => {}
                }
            }
            None
        })
        .collect()
}

/// One parameter of a `pub fn`: (function, parameter, type text, generics text), for every `pub fn` in `code`.
fn pub_fn_params(code: &str) -> Vec<(String, String, String, String)> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices("pub ") {
        let rest = &code[at + 4..];
        let rest = ["const ", "unsafe ", "extern ", "async "]
            .iter()
            .fold(rest, |r, q| r.strip_prefix(q).unwrap_or(r));
        let Some(rest) = rest.strip_prefix("fn ") else {
            continue;
        };
        let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        let mut rest = rest[name.len()..].trim_start();
        let mut generics = "";
        if rest.starts_with('<') {
            let Some(end) = matching(rest, '<', '>') else {
                continue;
            };
            generics = &rest[1..end];
            rest = rest[end + 1..].trim_start();
        }
        if !rest.starts_with('(') {
            continue;
        }
        let Some(end) = matching(rest, '(', ')') else {
            continue;
        };
        for param in split_top_level(&rest[1..end]) {
            let Some((pname, ptype)) = param.split_once(':') else {
                continue;
            };
            found.push((
                name.clone(),
                pname.trim().trim_start_matches("mut ").to_owned(),
                ptype.split_whitespace().collect::<Vec<_>>().join(" "),
                generics.to_owned(),
            ));
        }
    }
    found
}

/// Index of the bracket that closes the one `text` starts with. A `>` that belongs to `->` or `=>` does not count.
fn matching(text: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0_usize;
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close && !(close == '>' && (prev == '-' || prev == '=')) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        prev = c;
    }
    None
}

fn split_top_level(params: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut prev) = (0_i32, 0, ' ');
    for (i, c) in params.char_indices() {
        match c {
            '(' | '[' | '<' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if prev != '-' && prev != '=' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        prev = c;
    }
    if !params[start..].trim().is_empty() {
        out.push(&params[start..]);
    }
    out
}

/// `pub fn` parameters that look like a path and are text: (function, parameter, type).
fn raw_path_params(code: &str) -> Vec<(String, String, String)> {
    const TEXT: [&str; 6] = ["str", "String", "Path", "PathBuf", "OsStr", "OsString"];
    let is_text = |ty: &str| ty.split(|c: char| !is_ident(c)).any(|t| TEXT.contains(&t));
    pub_fn_params(code)
        .into_iter()
        .filter(|(_, name, ty, generics)| {
            let name = name.to_ascii_lowercase();
            let pathy = name.contains("path")
                || ["file", "filename", "file_name", "dir", "folder", "directory"].contains(&name.as_str());
            // `fn f<S: AsRef<str>>(path: S)` is as raw as `path: &str`: a bare type parameter counts when a bound names text.
            let bare_generic = ty.chars().all(is_ident) && is_text(generics);
            pathy && (is_text(ty) || bare_generic)
        })
        .map(|(f, p, t, _)| (f, p, t))
        .collect()
}

#[test]
fn forbid_unsafe_present() {
    let lib = fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    assert!(
        lib.contains("#![forbid(unsafe_code)]"),
        "lib.rs must forbid unsafe code"
    );
    for (file, code) in sources() {
        assert!(!contains_word(&code, "unsafe"), "{file} uses unsafe");
        // `unsafe_code` may appear only in the one forbid attribute.
        let without_forbid = code.replace("forbid(unsafe_code)", "");
        assert!(
            !without_forbid.contains("unsafe_code"),
            "{file} mentions the unsafe_code lint"
        );
    }
}

#[test]
fn no_hashmap_or_hashset_in_src() {
    for (file, code) in sources() {
        for word in ["HashMap", "HashSet", "RandomState"] {
            assert!(
                !contains_word(&code, word),
                "{file} uses {word}: its iteration order is random, use BTreeMap, BTreeSet or a sorted Vec"
            );
        }
    }
}

#[test]
fn no_clock_or_io_in_src() {
    for (file, code) in sources() {
        for banned in [
            "std::fs",
            "std::net",
            "std::process",
            "std::env",
            "std::time::SystemTime",
            "std::time::Instant",
        ] {
            assert!(!code.contains(banned), "{file} uses {banned}");
        }
        for word in ["SystemTime", "Instant", "tokio", "async", "await"] {
            assert!(!contains_word(&code, word), "{file} uses {word}");
        }
        for group in std_groups(&code) {
            for word in ["fs", "net", "process", "env", "path"] {
                assert!(
                    !contains_word(group, word),
                    "{file} imports std::{word} in a group"
                );
            }
        }
    }
}

#[test]
fn public_api_takes_no_raw_string_paths() {
    for (file, code) in sources() {
        // Paths are typed values; the standard library's path types are for code that touches the file system.
        for banned in ["std::path", "PathBuf", "OsStr"] {
            assert!(!code.contains(banned), "{file} uses {banned}");
        }
        let bad = raw_path_params(&code);
        assert!(
            bad.is_empty(),
            "{file}: public functions take paths as text: {bad:?}"
        );
    }
}

#[test]
fn code_only_blanks_comments_and_literals() {
    let src = r##"
// HashMap in a line comment
/* HashMap in a block /* nested HashMap */ comment */
/// HashMap in a doc comment
fn f<'a>(s: &'a str) -> char {
    let _a = "HashMap in a string \" with an escape";
    let _b = r#"HashMap in a "raw" string"#;
    let _c = '"';
    let _d = '\'';
    let _e = '\u{1F600}';
    let _f: HashMap<u8, u8> = HashMap::new();
    '"'
}
"##;
    let code = code_only(src);
    assert_eq!(
        code.matches("HashMap").count(),
        2,
        "only the real uses are left: {code}"
    );
    assert!(code.contains("fn f<'a>(s: &'a str)"), "lifetimes stay: {code}");
    assert_eq!(code.lines().count(), src.lines().count(), "line numbers are kept");
}

#[test]
fn the_scans_find_what_they_look_for() {
    assert!(contains_word("let x = unsafe { 1 };", "unsafe"));
    assert!(!contains_word("forbid(unsafe_code)", "unsafe"));
    assert!(!contains_word("my_async_fn()", "async"));
    assert_eq!(std_groups("use std::{io, fs::File};"), ["io, fs::File"]);
    assert_eq!(
        std_groups("use std::{io::{self, Read}, fmt};"),
        ["io::{self, Read}, fmt"]
    );

    let code = code_only(
        "pub fn a(path: &str) {}\n\
         pub fn b<T: Into<String>>(self, logical_path: T, n: usize) {}\n\
         pub fn c(map: Map<K, V>, file_name: String) {}\n\
         pub fn d(path: &NfsPath, name: &str) {}\n\
         pub fn e(f: impl Fn(&str) -> bool, paths: &[&str]) {}\n\
         pub(crate) fn f(path: &str) {}\n\
         fn g(path: &str) {}\n",
    );
    let found: Vec<(String, String)> = raw_path_params(&code)
        .into_iter()
        .map(|(f, p, _)| (f, p))
        .collect();
    assert_eq!(
        found,
        [
            ("a".to_owned(), "path".to_owned()),
            ("b".to_owned(), "logical_path".to_owned()),
            ("c".to_owned(), "file_name".to_owned()),
            ("e".to_owned(), "paths".to_owned()),
        ]
    );
}
