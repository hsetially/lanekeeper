//! Byte-faithful writes (T2, rule 11, D17).
//!
//! A person edits text in a browser, and the browser hands the service the whole new file as UTF-8 with LF line endings. The
//! file on NFS may be CRLF, may carry a byte order mark, may be UTF-16 or Latin-1. Writing the text as it came would change
//! every line ending of the file, and the diff of the write, the audit event and any later PR would show a rewrite of the whole
//! file instead of the lines that were changed.
//!
//! So before a write the content is put into the form of the file it replaces:
//!
//! - **Encoding and byte order mark** are the file's own (`detect_format`). The text is transcoded when it came in another
//!   encoding. A text that the file's encoding cannot hold (a euro sign in a Latin-1 file) is refused, never changed.
//! - **Line endings** are the file's dominant style (`dominant_eol`). A file that mixes styles is written back in the style most
//!   of its lines had: a file has one style, as the engine defines it.
//! - **Only an explicit `change_eol`** gives a file another style (D17). It also applies to a new file.
//! - **A new file** takes the style most common among the text files in its folder. With no folder to follow, or a folder that is
//!   split evenly, it is written as it came: nobody knows better.
//! - **When the file cannot be read back** (the registry, the blob store and the agent all failed to give the version the
//!   request expects) the content is written as it came, and only `change_eol` is applied. The agent still checks the hash.
//! - **Binary and denied content** is never touched.
//!
//! Everything here is CPU-bound and synchronous (rule 4): callers run it on the blocking pool.

use bytes::Bytes;
use domain::{ChangeEol, EolStyle, FileClass, FileEntry, FileFormat, NfsPath, Page, SwimlaneId, User};
use engine::{
    DecodeError, Limits, Newline, apply_eol, decode_text, detect_format, dominant_eol, encode_text,
};

use crate::pipeline::validate::{Checked, Old, check};
use crate::service::Inner;
use domain::RejectReason;

/// What a writer needs to know about the bytes of a file to write the next version in the same form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFormatFacts {
    /// The encoding, the byte order mark and the style as the engine names them (`Mixed` for a file that mixes).
    pub format: FileFormat,
    /// The line ending to write: the dominant one.
    pub newline: Newline,
}

impl FileFormatFacts {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self {
            format: detect_format(bytes),
            newline: dominant_eol(bytes),
        }
    }
}

/// What the new content takes its form from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Basis {
    /// The file being replaced.
    Existing(FileFormatFacts),
    /// A file that does not exist yet: the style of its folder, when the folder has one.
    New { folder: Option<Newline> },
    /// The file being replaced could not be read back.
    Unknown,
}

impl Basis {
    /// The basis for replacing `old`. `folder` is used only when the file is new.
    pub(crate) fn of(old: &Old, folder: Option<Newline>) -> Self {
        match old {
            Old::Known(bytes) => Self::Existing(FileFormatFacts::of(bytes)),
            Old::Absent => Self::New { folder },
            Old::Unknown => Self::Unknown,
        }
    }
}

/// Why content could not be put into the form of its file. None of these carries any of the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PrepareError {
    /// The bytes are not text in the encoding they declare.
    #[error("the content is not valid text")]
    Undecodable,
    /// The file's encoding cannot hold a character of the new text.
    #[error("the content has a character the file's encoding cannot hold")]
    Unrepresentable,
    /// The content, in the file's form, is larger than the size limit.
    #[error("the content is larger than the size limit")]
    TooLarge,
}

impl PrepareError {
    pub(crate) fn reason(self) -> RejectReason {
        match self {
            Self::Undecodable | Self::Unrepresentable => RejectReason::InvalidContent,
            Self::TooLarge => RejectReason::TooLarge,
        }
    }
}

fn newline_of(change: ChangeEol) -> Newline {
    match change {
        ChangeEol::Lf => Newline::Lf,
        ChangeEol::Crlf => Newline::Crlf,
    }
}

/// Put `incoming` into the form its target has. See the module documentation for the rules.
pub(crate) fn prepare_content(
    incoming: &Bytes,
    class: FileClass,
    basis: Basis,
    change_eol: Option<ChangeEol>,
    limits: &Limits,
) -> Result<Bytes, PrepareError> {
    if matches!(class, FileClass::Binary | FileClass::Denied) {
        return Ok(incoming.clone());
    }
    let came = FileFormatFacts::of(incoming);
    let (encoding, bom, style) = match basis {
        Basis::Existing(file) => (file.format.encoding, file.format.bom, Some(file.newline)),
        Basis::New { folder } => (came.format.encoding, came.format.bom, folder),
        Basis::Unknown => (came.format.encoding, came.format.bom, None),
    };
    let style = change_eol.map(newline_of).or(style);

    let body = if encoding == came.format.encoding && bom == came.format.bom {
        incoming.clone()
    } else {
        let text = decode_text(incoming, limits).map_err(|e| match e {
            DecodeError::TooLarge => PrepareError::TooLarge,
            _ => PrepareError::Undecodable,
        })?;
        let format = FileFormat {
            eol: text.format.eol,
            encoding,
            bom,
        };
        encode_text(&text.text, format, limits).map_err(|e| match e {
            engine::EncodeError::TooLarge => PrepareError::TooLarge,
            engine::EncodeError::Unrepresentable | engine::EncodeError::BadFormat => {
                PrepareError::Unrepresentable
            }
        })?
    };
    Ok(match style {
        Some(newline) => apply_eol(&body, newline),
        None => body,
    })
}

/// The votes of the text files of a folder: a file with one style votes for it, and a file that mixes styles, a binary, a denied or
/// a withheld file, and a directory vote for nothing.
#[derive(Default)]
struct Votes {
    crlf: usize,
    lf: usize,
}

impl Votes {
    fn add(&mut self, entries: &[FileEntry]) {
        for entry in entries {
            if entry.is_dir
                || entry.content_withheld
                || matches!(entry.class, FileClass::Binary | FileClass::Denied)
            {
                continue;
            }
            match entry.format.map(|f| f.eol) {
                Some(EolStyle::Crlf) => self.crlf += 1,
                Some(EolStyle::Lf) => self.lf += 1,
                Some(EolStyle::Mixed) | None => {}
            }
        }
    }

    fn style(&self) -> Option<Newline> {
        match self.crlf.cmp(&self.lf) {
            std::cmp::Ordering::Greater => Some(Newline::Crlf),
            std::cmp::Ordering::Less => Some(Newline::Lf),
            std::cmp::Ordering::Equal => None,
        }
    }
}

/// The line ending most of the text files in a folder have, or `None` when the folder has no text file or is split evenly.
pub(crate) fn folder_eol_style(entries: &[FileEntry]) -> Option<Newline> {
    let mut votes = Votes::default();
    votes.add(entries);
    votes.style()
}

/// The most pages of a folder read to find its style: 20 pages of 500 entries.
const MAX_FOLDER_PAGES: usize = 20;

/// The style of the folder a new file goes into, from the registry's pages of the folder (at most [`MAX_FOLDER_PAGES`]), so that a
/// big folder is judged by all of it and not by its first 500 entries. A registry that does not answer is no reason to refuse a
/// write: the file is then written as it came, and a registry that stops answering after some pages leaves the style those pages
/// show.
pub(crate) async fn folder_newline(
    hub: &Inner,
    user: &User,
    swimlane: &SwimlaneId,
    path: &NfsPath,
) -> Option<Newline> {
    let mut votes = Votes::default();
    let mut cursor = None;
    for _ in 0..MAX_FOLDER_PAGES {
        let page = Page::new(Page::MAX_LIMIT, cursor.take()).ok()?;
        let Ok(Ok(listed)) = hub
            .within(
                "folder_style",
                hub.registry.tree(user, swimlane, &path.parent(), page),
            )
            .await
        else {
            break;
        };
        votes.add(&listed.items);
        match listed.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    votes.style()
}

/// What `prepare_checked` is asked to write.
pub(crate) struct Incoming<'a> {
    pub path: &'a NfsPath,
    pub class: FileClass,
    pub content: &'a Bytes,
    /// The style of the folder, for a file that does not exist yet.
    pub folder: Option<Newline>,
    pub change_eol: Option<ChangeEol>,
    /// The size limit of the content once it is in the file's form.
    pub max_bytes: usize,
}

/// Put the content into the form of the file it replaces and validate the result: the order every executor follows, so what is
/// validated, diffed, scanned and written is one and the same set of bytes. Runs on the blocking pool.
pub(crate) fn prepare_checked(
    incoming: &Incoming<'_>,
    old: &Old,
    limits: &Limits,
) -> Result<(Bytes, Checked), RejectReason> {
    let basis = Basis::of(old, incoming.folder);
    let prepared = prepare_content(
        incoming.content,
        incoming.class,
        basis,
        incoming.change_eol,
        limits,
    )
    .map_err(PrepareError::reason)?;
    // Converting line endings can make content grow (a CR for every line).
    if prepared.len() > incoming.max_bytes {
        return Err(RejectReason::TooLarge);
    }
    let checked = check(incoming.path, incoming.class, &prepared, old, limits);
    Ok((prepared, checked))
}

/// A stored version put back (`revert`): written as it was, byte for byte, unless the request names a line ending style (D17). The
/// result is validated like any other write: a version that held a secret, or no longer parses, is not restored. Runs on the
/// blocking pool.
pub(crate) fn prepare_revert(
    path: &NfsPath,
    stored: &Bytes,
    change_eol: Option<ChangeEol>,
    max_bytes: usize,
    old: &Old,
    deny: &engine::DenyMatcher,
    limits: &Limits,
) -> Result<(Bytes, Checked), RejectReason> {
    let class = engine::classify(path, Some(stored), deny);
    let bytes = match change_eol {
        // The stored version is its own basis: its encoding and byte order mark stay, only the style changes.
        Some(_) => prepare_content(
            stored,
            class,
            Basis::Existing(FileFormatFacts::of(stored)),
            change_eol,
            limits,
        )
        .map_err(PrepareError::reason)?,
        None => stored.clone(),
    };
    if bytes.len() > max_bytes {
        return Err(RejectReason::TooLarge);
    }
    let checked = check(path, class, &bytes, old, limits);
    Ok((bytes, checked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DriftState, FileKind, FileRole, TextEncoding};
    use proptest::prelude::*;

    const LIMITS: Limits = Limits::DEFAULT;

    fn b(s: &[u8]) -> Bytes {
        Bytes::copy_from_slice(s)
    }

    fn facts(old: &[u8]) -> Basis {
        Basis::Existing(FileFormatFacts::of(old))
    }

    fn prep(incoming: &[u8], basis: Basis, change: Option<ChangeEol>) -> Result<Bytes, PrepareError> {
        prepare_content(&b(incoming), FileClass::Structured, basis, change, &LIMITS)
    }

    fn entry(name: &str, eol: Option<EolStyle>, class: FileClass) -> FileEntry {
        FileEntry {
            path: NfsPath::parse(name).unwrap(),
            is_dir: false,
            kind: FileKind::Base,
            class,
            role: FileRole::Resource,
            size: 1,
            hash: None,
            drift: DriftState::InSync,
            format: eol.map(|eol| FileFormat {
                eol,
                encoding: TextEncoding::Utf8,
                bom: false,
            }),
            content_withheld: false,
            attribution: None,
            severity: None,
            pickup_state: None,
            logical_file: None,
            tenant: None,
        }
    }

    #[test]
    fn lf_input_takes_the_crlf_of_the_file() {
        let out = prep(b"a: 1\nb: 2\n", facts(b"a: 0\r\nb: 0\r\n"), None).unwrap();
        assert_eq!(&out[..], b"a: 1\r\nb: 2\r\n");
    }

    #[test]
    fn crlf_input_takes_the_lf_of_an_lf_file() {
        let out = prep(b"a: 1\r\nb: 2\r\n", facts(b"a: 0\nb: 0\n"), None).unwrap();
        assert_eq!(&out[..], b"a: 1\nb: 2\n");
    }

    #[test]
    fn a_mixed_file_is_written_in_the_style_most_of_its_lines_had() {
        let out = prep(b"a\nb\nc\n", facts(b"a\r\nb\r\nc\n"), None).unwrap();
        assert_eq!(&out[..], b"a\r\nb\r\nc\r\n");
    }

    #[test]
    fn a_missing_final_newline_stays_missing() {
        let out = prep(b"a: 1\nb: 2", facts(b"a: 0\r\nb: 0"), None).unwrap();
        assert_eq!(&out[..], b"a: 1\r\nb: 2");
    }

    #[test]
    fn the_bom_of_the_file_is_kept_and_a_bom_it_lacks_is_dropped() {
        let with = prep(b"x: 2\n", facts(b"\xEF\xBB\xBFx: 1\n"), None).unwrap();
        assert_eq!(&with[..], b"\xEF\xBB\xBFx: 2\n");
        let without = prep(b"\xEF\xBB\xBFx: 2\n", facts(b"x: 1\n"), None).unwrap();
        assert_eq!(&without[..], b"x: 2\n");
        let kept = prep(b"\xEF\xBB\xBFx: 2\r\n", facts(b"\xEF\xBB\xBFx: 1\r\n"), None).unwrap();
        assert_eq!(&kept[..], b"\xEF\xBB\xBFx: 2\r\n");
    }

    #[test]
    fn text_is_transcoded_to_the_encoding_of_the_file() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("k=1\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let want: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("k=\u{e9}\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(
            &prep("k=\u{e9}\n".as_bytes(), facts(&utf16), None).unwrap()[..],
            &want[..]
        );
        // Latin-1 file: the text goes back as Latin-1.
        let out = prep("k=\u{e9}\n".as_bytes(), facts(b"k=\xE9\n"), None).unwrap();
        assert_eq!(&out[..], b"k=\xE9\n");
        // And a UTF-8 file gets UTF-8 from Latin-1 input.
        let out = prep(b"k=\xE9\n", facts(b"k=1\n"), None).unwrap();
        assert_eq!(&out[..], "k=\u{e9}\n".as_bytes());
    }

    #[test]
    fn text_the_encoding_cannot_hold_is_refused_without_the_text_in_the_error() {
        let err = prep("k=\u{20ac}secret-marker\n".as_bytes(), facts(b"k=\xE9\n"), None).unwrap_err();
        assert_eq!(err, PrepareError::Unrepresentable);
        assert_eq!(err.reason(), RejectReason::InvalidContent);
        assert!(!format!("{err} {err:?}").contains("secret-marker"));
    }

    #[test]
    fn undecodable_input_is_refused() {
        // A UTF-16 mark with an odd number of bytes after it, into a UTF-8 file.
        let err = prep(b"\xFF\xFEa", facts(b"k=1\n"), None).unwrap_err();
        assert_eq!(err, PrepareError::Undecodable);
    }

    #[test]
    fn change_eol_is_the_only_thing_that_changes_the_style() {
        let crlf_file = facts(b"a\r\nb\r\n");
        assert_eq!(&prep(b"a\nb\n", crlf_file, None).unwrap()[..], b"a\r\nb\r\n");
        assert_eq!(
            &prep(b"a\r\nb\r\n", crlf_file, Some(ChangeEol::Lf)).unwrap()[..],
            b"a\nb\n"
        );
        let lf_file = facts(b"a\nb\n");
        assert_eq!(
            &prep(b"a\nb\n", lf_file, Some(ChangeEol::Crlf)).unwrap()[..],
            b"a\r\nb\r\n"
        );
        // It also applies to a new file, over the folder's style, and to a file that cannot be read back.
        let new = Basis::New {
            folder: Some(Newline::Crlf),
        };
        assert_eq!(&prep(b"a\nb\n", new, Some(ChangeEol::Lf)).unwrap()[..], b"a\nb\n");
        assert_eq!(
            &prep(b"a\nb\n", Basis::Unknown, Some(ChangeEol::Crlf)).unwrap()[..],
            b"a\r\nb\r\n"
        );
    }

    #[test]
    fn a_new_file_follows_its_folder_or_is_written_as_it_came() {
        let crlf = Basis::New {
            folder: Some(Newline::Crlf),
        };
        assert_eq!(&prep(b"a\nb\n", crlf, None).unwrap()[..], b"a\r\nb\r\n");
        let lf = Basis::New {
            folder: Some(Newline::Lf),
        };
        assert_eq!(&prep(b"a\r\nb\r\n", lf, None).unwrap()[..], b"a\nb\n");
        let none = Basis::New { folder: None };
        assert_eq!(&prep(b"a\r\nb\n", none, None).unwrap()[..], b"a\r\nb\n");
        assert_eq!(&prep(b"a\r\nb\n", Basis::Unknown, None).unwrap()[..], b"a\r\nb\n");
    }

    #[test]
    fn binary_and_denied_content_is_untouched() {
        let raw = b"\xEF\xBB\xBFBM\x00\r\n\n\r";
        for class in [FileClass::Binary, FileClass::Denied] {
            let out =
                prepare_content(&b(raw), class, facts(b"a\r\nb\r\n"), Some(ChangeEol::Lf), &LIMITS).unwrap();
            assert_eq!(&out[..], raw);
        }
    }

    #[test]
    fn an_already_conforming_input_is_returned_without_a_copy() {
        let incoming = b(b"a: 1\r\nb: 2\r\n");
        let out = prepare_content(
            &incoming,
            FileClass::Structured,
            facts(b"x\r\ny\r\n"),
            None,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(out.as_ptr(), incoming.as_ptr());
    }

    #[test]
    fn the_folder_style_is_the_majority_of_its_text_files() {
        use EolStyle::{Crlf, Lf, Mixed};
        let t = FileClass::Structured;
        let style = |v: Vec<FileEntry>| folder_eol_style(&v);
        assert_eq!(
            style(vec![
                entry("a", Some(Crlf), t),
                entry("b", Some(Crlf), t),
                entry("c", Some(Lf), t)
            ]),
            Some(Newline::Crlf)
        );
        assert_eq!(
            style(vec![entry("a", Some(Lf), t), entry("b", Some(Crlf), t)]),
            None
        );
        assert_eq!(style(vec![]), None);
        // Mixed files, files with no format and binary files do not vote.
        assert_eq!(
            style(vec![
                entry("a", Some(Mixed), t),
                entry("b", None, t),
                entry("c", Some(Crlf), FileClass::Binary),
                entry("d", Some(Lf), t)
            ]),
            Some(Newline::Lf)
        );
        let mut dir = entry("d", Some(Crlf), t);
        dir.is_dir = true;
        let mut withheld = entry("w", Some(Crlf), t);
        withheld.content_withheld = true;
        assert_eq!(style(vec![dir, withheld]), None);
    }

    #[test]
    fn content_that_grows_past_the_limit_is_refused() {
        let limits = Limits {
            input_bytes: 20,
            ..Limits::DEFAULT
        };
        let old = Old::Known(b(b"x\r\n"));
        let path = NfsPath::parse("a.txt").unwrap();
        let content = b(b"a\nb\nc\nd\ne\n");
        let incoming = |max_bytes| Incoming {
            path: &path,
            class: FileClass::Text,
            content: &content,
            folder: None,
            change_eol: None,
            max_bytes,
        };
        // 10 bytes as LF, 15 as CRLF.
        assert_eq!(
            prepare_checked(&incoming(12), &old, &limits).unwrap_err(),
            RejectReason::TooLarge
        );
        let (bytes, checked) = prepare_checked(&incoming(15), &old, &limits).unwrap();
        assert_eq!(&bytes[..], b"a\r\nb\r\nc\r\nd\r\ne\r\n");
        assert!(checked.reject.is_none());
    }

    /// A file as bytes, in the given form, built without the functions under test.
    fn file_bytes(text: &str, utf16: bool, bom: bool, crlf: bool) -> Vec<u8> {
        let text = if crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_owned()
        };
        if utf16 {
            let mut out = vec![0xFF, 0xFE];
            out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            out
        } else {
            let mut out = if bom { vec![0xEF, 0xBB, 0xBF] } else { Vec::new() };
            out.extend_from_slice(text.as_bytes());
            out
        }
    }

    proptest! {
        /// For any file in any form, input that differs from the file in one line (and uses LF, no mark, UTF-8) comes out as
        /// the file's form with that one line changed: the same bytes as the file everywhere else.
        #[test]
        fn prepare_content_preserves_style(
            lines in proptest::collection::vec("[a-z0-9: ]{0,12}", 1..20),
            at in 0usize..20,
            replacement in "[A-Z0-9=]{1,10}",
            utf16 in any::<bool>(),
            bom in any::<bool>(),
            crlf in any::<bool>(),
            final_newline in any::<bool>(),
        ) {
            // A file with one empty line and no terminator has no line ending, so its style is unknowable.
            prop_assume!(lines.len() > 1 || final_newline);
            let at = at % lines.len();
            let join = |lines: &[String]| {
                let mut t = lines.join("\n");
                if final_newline { t.push('\n'); }
                t
            };
            let old_text = join(&lines);
            let mut changed = lines.clone();
            changed[at] = replacement;
            let new_text = join(&changed);

            let old = file_bytes(&old_text, utf16, bom, crlf);
            let expected = file_bytes(&new_text, utf16, bom, crlf);
            let got = prepare_content(&b(new_text.as_bytes()), FileClass::Structured, facts(&old), None, &LIMITS).unwrap();
            prop_assert_eq!(&got[..], &expected[..]);
        }

        /// Applying the preparation twice changes nothing.
        #[test]
        fn preparation_is_idempotent(
            lines in proptest::collection::vec("[a-z0-9: ]{0,12}", 1..10),
            crlf in any::<bool>(),
            explicit in proptest::option::of(any::<bool>()),
        ) {
            let old = file_bytes(&(lines.join("\n") + "\n"), false, false, crlf);
            let change = explicit.map(|c| if c { ChangeEol::Crlf } else { ChangeEol::Lf });
            let once = prepare_content(&b((lines.join("\r\n") + "\n").as_bytes()), FileClass::Text, facts(&old), change, &LIMITS).unwrap();
            let twice = prepare_content(&once, FileClass::Text, facts(&old), change, &LIMITS).unwrap();
            prop_assert_eq!(once, twice);
        }
    }
}
