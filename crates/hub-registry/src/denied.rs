//! Denied files (T9, D79, D104; S4, S10, S14, S17): the one rule that says which files are denied, and where the registry holds to it.
//!
//! A denied file is one whose content the hub must never read beyond hashing, never serve and never write (D79: the keystores, keys and
//! private-looking names). The agent applies the deny globs, reports such a file by name, size and hash with the flag set, and sends no bytes. The
//! registry records that (class `denied`, the flag, no blob) and then withholds: whatever it is asked, the answer is a name, a size, a hash and
//! "content withheld", with no bytes.
//!
//! # The rule
//!
//! [`is_denied`]: a file is denied when **any** of these says so:
//!
//! 1. the stored flag (`file_state.denied`), which ingestion sets from the agent's report or from the hub's own globs, whichever says denied
//!    (`ingest::validate`: an agent cannot make a file readable by forgetting to mark it);
//! 2. its class (`denied`), which says the same thing and is read the same way (a row that disagrees with itself is denied);
//! 3. the hub's own deny globs, now: the engine's defaults and the admin's `extra_deny_globs`. This is defence in depth, for the time
//!    between the hub starting with another list and the agents' next scan, when the rows still say what they said.
//!
//! A change of the flag with the same content is a change of the file (`ingest::plan`: the agent's tree names a change of the flag as a change,
//! so the same hash with another flag is recorded and audited): a file that becomes denied is denied from then on, and one that stops being
//! denied is an ordinary file again.
//!
//! # Where it holds
//!
//! | Read | Where it is decided |
//! |---|---|
//! | tree and drift list: class `denied`, `content_withheld` | [`is_denied`] in `read::tree::file_entry` |
//! | content, from the file, its baseline, any version it has had, Git | [`is_denied`] in `read::content` (a denied file is not read from Git either) |
//! | compare, in either side; diff hunks; setting changes | `LayerRow.denied` (below) and the Git side's class: nothing but hashes and sizes of a denied side |
//! | effective config, resources, grid | `LayerRow.denied` (below): a denied file has none |
//! | settings search | the places of a setting are the files that have it **now** and are not denied (`NOT denied` in the statement), and the hub's globs on the page |
//! | text search | the corpus has no denied file (`NOT denied`, and the engine's search applies the hub's globs to every path) |
//! | history | hashes, sizes, sources and attribution: never content |
//! | served | a denied file is not asked for from the agent: `withheld` |
//! | the blob endpoint | `RegistryBlobPolicy`: a hash that a denied file has or has had (the same rule, over the paths that have had it, also for a deleted file) is refused as an unknown one is |
//!
//! `LayerRow.denied` is set where the row is read (`read::effective::layer_row`) from [`is_denied`], so the comparisons, the effective config, the
//! resources and the grid, which all read their files through it, decide the same way.
//!
//! # What this does not do
//!
//! - **A file that was an ordinary file keeps its old versions in the store.** Their bytes were stored when nothing said they were secret.
//!   Nothing reads them out for a denied path (the content of a path by hash serves only a version that path has had, and withholds it when
//!   the path is denied), the blob endpoint refuses them, and the retention job collects them when no rule keeps them; nothing deletes them
//!   at once.
//! - **A comparison of two Git refs** (`git:base@a` against `git:base@b`) is of the repository, not of a swimlane, and no row says a path is
//!   denied there: only the hub's globs and the class of the bytes do. The Git copy of a file a swimlane's agent denied by a flag that the
//!   hub's globs do not have is readable that way. Config in Git is not secret (D18) and the deny list is for what is not in Git.

use domain::{FileClass, NfsPath};
use engine::DenyMatcher;

/// Whether a file's content must never be read, served or written. See the module documentation for the three things that say so.
pub fn is_denied(deny: &DenyMatcher, path: &NfsPath, flagged: bool, class: FileClass) -> bool {
    flagged || class == FileClass::Denied || deny.is_denied(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> NfsPath {
        NfsPath::parse(p).unwrap()
    }

    #[test]
    fn any_one_of_the_flag_the_class_and_the_hubs_globs_denies() {
        let globs = DenyMatcher::with_globs(["*secret*"]).unwrap();
        let none = DenyMatcher::default();
        // Not denied by anything.
        assert!(!is_denied(
            &none,
            &path("app/core.yml"),
            false,
            FileClass::Structured
        ));
        // The flag, whatever the class says.
        assert!(is_denied(
            &none,
            &path("app/core.yml"),
            true,
            FileClass::Structured
        ));
        // The class, whatever the flag says: a row that disagrees with itself is read as denied.
        assert!(is_denied(&none, &path("app/core.yml"), false, FileClass::Denied));
        // The hub's own glob, whatever the row says.
        assert!(is_denied(
            &globs,
            &path("app/my-secret.yml"),
            false,
            FileClass::Structured
        ));
        assert!(!is_denied(
            &globs,
            &path("app/core.yml"),
            false,
            FileClass::Structured
        ));
        // The engine's defaults are in every matcher.
        assert!(is_denied(&none, &path("keys/server.pem"), false, FileClass::Text));
        assert!(is_denied(&none, &path("a/b.jks"), false, FileClass::Binary));
    }
}
