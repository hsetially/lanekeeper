//! File classes and the deny globs (D79, S11).
//!
//! A file is `structured`, `text` or `binary`, and decides how it is compared (`docs/domain-model.md`, "Files on NFS"):
//!
//! | Class | Files |
//! |---|---|
//! | `structured` | `.yml`, `.yaml`, `.json`, `.properties` |
//! | `text` | `.xsl`, `.xml`, `.txt`, and extension-less files that are UTF-8 |
//! | `binary` | everything else |
//!
//! A file the deny globs match is `denied` first, whatever it is named otherwise. Its content is never read for anyone.
//! The agent applies the same globs on the NFS side and tells the hub; the hub applies them here to the Git side, so a
//! key committed to a repository is withheld too.

use std::fmt;

use domain::{FileClass, NfsPath};
use globset::{Candidate, Glob, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::error::DenyError;

/// The globs that are always in force (D79). The agent has the same list; a change must be made on both sides.
pub const DEFAULT_DENY_GLOBS: [&str; 7] = [
    "*.jks",
    "*.p12",
    "*.pfx",
    "*.pem",
    "*.key",
    "*.keystore",
    "*private*",
];

const STRUCTURED_EXTENSIONS: [&str; 4] = ["yml", "yaml", "json", "properties"];
const TEXT_EXTENSIONS: [&str; 3] = ["xsl", "xml", "txt"];
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// The deny globs, compiled. Matching ignores ASCII case, because `SERVER.KEY` is as much a key as `server.key`.
///
/// A glob is matched against a file's whole path from the root and, when it has no slash, against its name alone. `*`
/// crosses `/`, so `*.pem` and `*private*` are found at any depth, and a glob with a slash such as `secrets/*` is found
/// where it says.
#[derive(Clone)]
pub struct DenyMatcher {
    /// `None` only if the built-in globs failed to compile, which a test rules out. Then everything is denied.
    set: Option<GlobSet>,
}

impl DenyMatcher {
    /// The most globs `with_globs` takes.
    pub const MAX_EXTRA_GLOBS: usize = 1_000;
    /// The longest glob `with_globs` takes, in bytes.
    pub const MAX_GLOB_BYTES: usize = 512;

    /// The built-in globs and the given ones. The given ones add to the built-in ones and can never take one away.
    ///
    /// # Errors
    ///
    /// `TooManyGlobs`, `GlobTooLong`, or `InvalidGlob` for a glob that does not compile. The error does not contain the
    /// glob.
    pub fn with_globs<'a>(extra: impl IntoIterator<Item = &'a str>) -> Result<Self, DenyError> {
        let mut globs = Vec::new();
        for text in extra {
            if globs.len() == Self::MAX_EXTRA_GLOBS {
                return Err(DenyError::TooManyGlobs);
            }
            if text.len() > Self::MAX_GLOB_BYTES {
                return Err(DenyError::GlobTooLong);
            }
            globs.push(glob(text).map_err(|_| DenyError::InvalidGlob)?);
        }
        compile(globs)
            .map(|set| Self { set: Some(set) })
            .map_err(|_| DenyError::InvalidGlob)
    }

    /// True when the path is denied.
    pub fn is_denied(&self, path: &NfsPath) -> bool {
        let Some(set) = &self.set else {
            return true;
        };
        let whole = Candidate::new(path.as_str());
        if set.is_match_candidate(&whole) {
            return true;
        }
        // A glob with no slash is also matched against the name alone.
        let name = path.file_name();
        name.len() != path.as_str().len() && set.is_match_candidate(&Candidate::new(name))
    }
}

impl Default for DenyMatcher {
    /// The built-in globs only.
    fn default() -> Self {
        Self {
            set: compile(Vec::new()).ok(),
        }
    }
}

impl fmt::Debug for DenyMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The globs are admin configuration; they are not printed.
        f.debug_struct("DenyMatcher")
            .field("globs", &self.set.as_ref().map(GlobSet::len))
            .finish()
    }
}

fn glob(text: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(text).case_insensitive(true).build()
}

fn compile(extra: Vec<Glob>) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for text in DEFAULT_DENY_GLOBS {
        builder.add(glob(text)?);
    }
    for g in extra {
        builder.add(g);
    }
    builder.build()
}

/// The class of a file.
///
/// `content` is the file's bytes, when the caller has them. It matters for an extension-less file only: it is `text` when
/// the bytes are UTF-8 (a byte order mark is allowed) with no NUL, and `binary` otherwise. Without the bytes there is
/// nothing to decide with, and `binary` (compare by hash) is the safe answer.
pub fn classify(path: &NfsPath, content: Option<&[u8]>, deny: &DenyMatcher) -> FileClass {
    if deny.is_denied(path) {
        return FileClass::Denied;
    }
    match extension(path.file_name()) {
        Some(ext) if is_one_of(&STRUCTURED_EXTENSIONS, ext) => FileClass::Structured,
        Some(ext) if is_one_of(&TEXT_EXTENSIONS, ext) => FileClass::Text,
        None if content.is_some_and(is_utf8_text) => FileClass::Text,
        Some(_) | None => FileClass::Binary,
    }
}

/// The part of a file name after its last dot. A name that only starts with a dot (`.profile`) or ends in one has none.
fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then_some(ext)
}

fn is_one_of(list: &[&str], ext: &str) -> bool {
    list.iter().any(|known| known.eq_ignore_ascii_case(ext))
}

fn is_utf8_text(content: &[u8]) -> bool {
    let body = content.strip_prefix(&UTF8_BOM).unwrap_or(content);
    !body.contains(&0) && std::str::from_utf8(body).is_ok()
}
