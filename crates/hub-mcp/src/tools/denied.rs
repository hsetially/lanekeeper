//! Denied files (D79, S14b, S17): known by name, size and hash only.
//!
//! The agent has deny globs (`*.jks`, `*.p12`, `*.pfx`, `*.pem`, `*.key`, `*.keystore`, `*private*`) and never reads, hashes beyond, or
//! sends the content of a file that matches. So the hub holds a denied file's name, size and hash, and a port is expected to say so
//! (the class `denied`, or the flag that withholds the content). That is the rule. This is the floor under it: the same built-in
//! globs, applied here by name to every path a tool is about to show the content of, so that a port that forgot, an agent with an
//! older list, or a fixture that sent bytes anyway does not make a key readable. The globs the agent is configured with in addition
//! can only add to the built-in ones, never take one away (the engine's matcher), and arrive here as the port's class and flag.
//!
//! What a tool does with a denied file depends on what its answer is made of:
//!
//! - an answer about one file (`read_file`, `get_served_config`) says `content_withheld` and shows the name, size and hash;
//! - a comparison (`compare`) says the class is `denied` and keeps the hashes and sizes, with no changes and no lines;
//! - a list of files (`list_files`, `get_drift`) shows the entry with `content_withheld` and the class `denied`;
//! - a list of hits (`search_settings`, `search_text`) leaves out a hit in a denied file, as if the file had no content, which for
//!   the agent it does not;
//! - a question that is only about the content of one denied file (`get_effective_config`, `compare_grid`, and a write of it) is
//!   `unprocessable` with the reason `denied`, before the port is asked.

use domain::{LogicalFile, NfsPath};
use engine::DenyMatcher;

/// The built-in deny globs, compiled once.
#[derive(Clone, Debug)]
pub struct Denied(DenyMatcher);

impl Default for Denied {
    fn default() -> Self {
        Self::new()
    }
}

impl Denied {
    #[must_use]
    pub fn new() -> Self {
        Self(DenyMatcher::default())
    }

    /// Whether the name of `path` is a denied file's.
    #[must_use]
    pub fn path(&self, path: &NfsPath) -> bool {
        self.0.is_denied(path)
    }

    /// As [`Denied::path`], for a file named without its tenant suffix.
    #[must_use]
    pub fn file(&self, file: &LogicalFile) -> bool {
        self.path(file.as_path())
    }
}
