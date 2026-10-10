//! Deny globs (T11, D79, S17): keystores, keys and private material are hashed and never read for anyone else.
//!
//! A path that matches the [`DenyList`] is a **denied** file. The agent still tracks it (name, size and a streamed
//! SHA-256, so that the hub sees it exists and sees it change), and that is all:
//!
//! - the walker marks its leaf `denied`, and the delta builder sends the entry with no bytes;
//! - `ReadFile`, `WriteFile` and `DeleteFile` on it are refused with `DENIED` before anything is opened, so the answer
//!   never carries a hash either (a conflict would);
//! - the spool never stores bytes for it, and a record written before the path became denied is scrubbed when it is
//!   replayed, so the bytes never reach the connection;
//! - nothing logs its content (nothing logs content at all, S10).
//!
//! # Where the globs come from
//!
//! [`DEFAULT_DENY_GLOBS`] are built in. The hub's `AgentConfig` adds more. The hub can add and take back its own, but
//! it can never remove a default: the defaults are not part of what [`DenyList::set_hub_globs`] replaces, and no glob
//! syntax can subtract from another (globset has no negation). A hub glob that does not compile is dropped and counted
//! ([`Applied::rejected`]); the rest of the hub's globs are still used. A hub that sends too many, or one that is too
//! long, is cut off at a bound (rule 5).
//!
//! # What matches
//!
//! A glob is matched against the file's name and against its whole path from the root, so `*.pem` and `*private*` are
//! found at any depth, and `secrets/*` is found where it says. Matching ignores ASCII case, because `SERVER.KEY` is as
//! much a key as `server.key`. `*` crosses `/`, so `*private*` also denies everything below a directory with `private`
//! in its name.
//!
//! One list is shared by everything that touches a file's bytes (the walker, the delta builder, file operations and the
//! spool), through cheap clones. A change to the hub's globs is seen by all of them at once, and a walk takes one
//! [`DenySnapshot`] at its start so that it does not change its mind half way.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use domain::{ScanDelta, ShortText};
use globset::{Candidate, Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// The globs that are always in force (D79). The hub cannot remove them.
pub const DEFAULT_DENY_GLOBS: [&str; 7] = [
    "*.jks",
    "*.p12",
    "*.pfx",
    "*.pem",
    "*.key",
    "*.keystore",
    "*private*",
];

/// The most globs the hub can add. It is the contract's own limit for a configuration list, so every list the wire
/// accepts is used whole.
pub const MAX_HUB_GLOBS: usize = proto::limits::MAX_CONFIG_ITEMS;
/// The longest glob that is compiled. A longer one is dropped (it costs compile time and means nothing a short one
/// could not say).
pub const MAX_GLOB_BYTES: usize = 512;
/// Globs compiled into one set. A set that is too big to compile falls back to one set per glob.
const CHUNK: usize = 64;

/// What applying the hub's globs did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    /// The globs now in force differ from the ones before: files may have been denied or released.
    pub changed: bool,
    /// Hub globs that were not used: too many, too long, or not a valid glob.
    pub rejected: usize,
}

/// The compiled globs. Immutable; a change makes a new one.
struct Compiled {
    /// `None` only if the built-in globs failed to compile, which the tests rule out; then everything is denied.
    defaults: Option<GlobSet>,
    hub: Vec<GlobSet>,
    /// The hub globs in use, sorted and without repeats, to tell whether a new list changes anything.
    hub_globs: Vec<String>,
    /// Globs the list was given and did not use: too many, too long, or not a glob.
    rejected_input: usize,
    /// Globs that were valid alone and still could not be compiled into a set.
    failed_build: usize,
}

impl Compiled {
    fn new(hub_globs: Vec<(String, Glob)>, rejected_input: usize) -> Self {
        let (sets, failed_build) = build_sets(&hub_globs);
        Self {
            defaults: build_defaults(),
            hub: sets,
            hub_globs: hub_globs.into_iter().map(|(text, _)| text).collect(),
            rejected_input,
            failed_build,
        }
    }

    fn rejected(&self) -> usize {
        self.rejected_input + self.failed_build
    }

    fn is_denied_parts(&self, name: &str, rel: &str) -> bool {
        let Some(defaults) = &self.defaults else {
            return true;
        };
        // The built-in globs all start with `*`, which crosses `/`, so the whole path decides: a name that matches makes
        // the path match, and a directory name that matches (`*private*`) makes everything below it match. One match per
        // file is what the walker, which asks for every file on every walk, can afford (P4).
        let path = Candidate::new(rel);
        if defaults.is_match_candidate(&path) {
            return true;
        }
        if self.hub.is_empty() {
            return false;
        }
        // The hub's globs may be a bare name (`exact-name`), which only the name finds.
        let name = (name.len() != rel.len()).then(|| Candidate::new(name));
        self.hub.iter().any(|set| {
            set.is_match_candidate(&path) || name.as_ref().is_some_and(|n| set.is_match_candidate(n))
        })
    }
}

fn glob(text: &str) -> Result<Glob, globset::Error> {
    GlobBuilder::new(text).case_insensitive(true).build()
}

fn build_defaults() -> Option<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for text in DEFAULT_DENY_GLOBS {
        builder.add(glob(text).ok()?);
    }
    builder.build().ok()
}

/// Compile the globs in chunks. A chunk that does not build as a whole is built glob by glob, and a glob that still
/// does not build is counted.
fn build_sets(globs: &[(String, Glob)]) -> (Vec<GlobSet>, usize) {
    let mut sets = Vec::new();
    let mut failed = 0;
    for chunk in globs.chunks(CHUNK) {
        let mut builder = GlobSetBuilder::new();
        for (_, glob) in chunk {
            builder.add(glob.clone());
        }
        match builder.build() {
            Ok(set) => sets.push(set),
            Err(_) => {
                for (_, glob) in chunk {
                    let mut one = GlobSetBuilder::new();
                    one.add(glob.clone());
                    match one.build() {
                        Ok(set) => sets.push(set),
                        Err(_) => failed += 1,
                    }
                }
            }
        }
    }
    (sets, failed)
}

/// Pick the hub globs that can be used: at most [`MAX_HUB_GLOBS`], each at most [`MAX_GLOB_BYTES`] and valid; sorted,
/// without repeats. Also returns how many were left out.
fn accept<'a>(hub: impl IntoIterator<Item = &'a str>) -> (Vec<(String, Glob)>, usize) {
    let mut kept: std::collections::BTreeMap<String, Glob> = std::collections::BTreeMap::new();
    let mut rejected = 0;
    for (i, text) in hub.into_iter().enumerate() {
        if i >= MAX_HUB_GLOBS || text.len() > MAX_GLOB_BYTES {
            rejected += 1;
            continue;
        }
        match glob(text) {
            Ok(compiled) => {
                kept.insert(text.to_owned(), compiled);
            }
            Err(_) => rejected += 1,
        }
    }
    (kept.into_iter().collect(), rejected)
}

/// The globs one walk uses, fixed at its start.
#[derive(Clone)]
pub struct DenySnapshot(Arc<Compiled>);

impl DenySnapshot {
    /// Is the file at `rel` (a path from the root, `/`-separated) denied?
    pub fn is_denied(&self, rel: &str) -> bool {
        self.0.is_denied_parts(name_of(rel), rel)
    }

    /// As [`DenySnapshot::is_denied`], when the caller already has the name.
    pub fn is_denied_parts(&self, name: &str, rel: &str) -> bool {
        self.0.is_denied_parts(name, rel)
    }
}

impl fmt::Debug for DenySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DenySnapshot")
            .field("hub_globs", &self.0.hub_globs.len())
            .finish_non_exhaustive()
    }
}

fn name_of(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// The deny globs in force, shared by everything that reads a file's bytes. Cheap to clone; all clones are one list.
#[derive(Clone)]
pub struct DenyList {
    current: Arc<RwLock<Arc<Compiled>>>,
}

impl fmt::Debug for DenyList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let current = self.compiled();
        f.debug_struct("DenyList")
            .field("hub_globs", &current.hub_globs.len())
            .field("rejected", &current.rejected())
            .finish_non_exhaustive()
    }
}

impl Default for DenyList {
    /// The built-in globs and nothing from the hub.
    fn default() -> Self {
        Self::new(&[])
    }
}

impl DenyList {
    /// The built-in globs plus `hub`. Globs that cannot be used are dropped; [`DenyList::rejected`] says how many.
    pub fn new(hub: &[&str]) -> Self {
        let (accepted, rejected) = accept(hub.iter().copied());
        Self {
            current: Arc::new(RwLock::new(Arc::new(Compiled::new(accepted, rejected)))),
        }
    }

    fn compiled(&self) -> Arc<Compiled> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Replace the hub's part of the list with `hub`. The built-in globs are untouched, whatever `hub` holds.
    pub fn set_hub_globs(&self, hub: &[&str]) -> Applied {
        self.apply(accept(hub.iter().copied()))
    }

    /// As [`DenyList::set_hub_globs`], for the globs of an `AgentConfig`.
    pub fn set_hub_texts(&self, hub: &[ShortText]) -> Applied {
        self.apply(accept(hub.iter().map(ShortText::as_str)))
    }

    fn apply(&self, (accepted, rejected_input): (Vec<(String, Glob)>, usize)) -> Applied {
        let current = self.compiled();
        let same_globs = current.hub_globs.len() == accepted.len()
            && current
                .hub_globs
                .iter()
                .zip(&accepted)
                .all(|(have, (want, _))| have == want);
        if same_globs && current.rejected_input == rejected_input {
            // The same list again (every connection sends it): keep the compiled sets.
            return Applied {
                changed: false,
                rejected: current.rejected(),
            };
        }
        // Compiled outside the lock: readers wait only for the swap. The one writer is the scanner, on the hub's
        // configuration, so two applies do not race for different lists.
        let next = Compiled::new(accepted, rejected_input);
        let rejected = next.rejected();
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        Applied {
            changed: !same_globs,
            rejected,
        }
    }

    /// Hub globs that were left out of the list in force.
    pub fn rejected(&self) -> usize {
        self.compiled().rejected()
    }

    /// The hub's globs in force, sorted. For tests and diagnostics.
    pub fn hub_globs(&self) -> Vec<String> {
        self.compiled().hub_globs.clone()
    }

    /// The globs as they are now, to use for as long as the caller likes (one walk).
    pub fn snapshot(&self) -> DenySnapshot {
        DenySnapshot(self.compiled())
    }

    /// Is the file at `rel` (a path from the root) denied?
    pub fn is_denied(&self, rel: &str) -> bool {
        self.compiled().is_denied_parts(name_of(rel), rel)
    }

    /// Strip the bytes from every entry of `delta` whose path is denied now, and mark it denied. The name, size and
    /// hash stay. Returns how many entries changed.
    ///
    /// This is the last guard before a message is stored or sent: the walker and the builder already keep denied files
    /// out, so it matters for a record written before the hub added a glob, and as a net under everything else.
    pub fn scrub(&self, delta: &mut ScanDelta) -> usize {
        let snapshot = self.snapshot();
        let mut changed = 0;
        for entry in &mut delta.entries {
            if (entry.bytes.is_some() || !entry.denied) && snapshot.is_denied(entry.path.as_str()) {
                entry.bytes = None;
                entry.denied = true;
                changed += 1;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use domain::{ContentHash, NfsPath, ScanEntry, Timestamp};
    use proptest::prelude::*;

    use super::*;

    /// A file name for each built-in glob, and where it may sit.
    const DENIED_SAMPLES: [&str; 14] = [
        "server.jks",
        "svc/store.p12",
        "a/b/c/client.pfx",
        "tls.pem",
        "svc/sub/tls.key",
        "trust.keystore",
        "svc/private-settings.yml",
        "private/notes.yml",
        "svc/my-private.txt",
        "SERVER.KEY",
        "svc/Store.JKS",
        "svc/Tls.Pem",
        "svc/PRIVATE.yml",
        "deep/er/and/deeper/id_rsa.key",
    ];

    const ALLOWED_SAMPLES: [&str; 8] = [
        "svc/app.yml",
        "svc/application-sit1.properties",
        "svc/keystore-notes.txt",
        "svc/pemfile.yml",
        "svc/key.yml",
        "svc/jks",
        "logo.bmp",
        "svc/public.yml",
    ];

    fn assert_defaults_hold(list: &DenyList) {
        for sample in DENIED_SAMPLES {
            assert!(list.is_denied(sample), "{sample} must be denied");
        }
    }

    #[test]
    fn default_globs_compile_and_deny_their_samples() {
        // If the built-ins ever failed to compile, everything would be denied; this proves they do not.
        assert!(build_defaults().is_some());
        let list = DenyList::default();
        assert_defaults_hold(&list);
        for sample in ALLOWED_SAMPLES {
            assert!(!list.is_denied(sample), "{sample} must not be denied");
        }
        assert_eq!(list.rejected(), 0);
        assert!(list.hub_globs().is_empty());
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        assert_eq!(
            DEFAULT_DENY_GLOBS,
            [
                "*.jks",
                "*.p12",
                "*.pfx",
                "*.pem",
                "*.key",
                "*.keystore",
                "*private*"
            ]
        );
    }

    #[test]
    fn matching_ignores_ascii_case_and_looks_at_the_name_and_the_path() {
        let list = DenyList::new(&["secrets/*", "exact-name"]);
        assert!(list.is_denied("secrets/anything.yml"));
        assert!(list.is_denied("SECRETS/anything.yml"));
        assert!(list.is_denied("svc/exact-name"));
        assert!(!list.is_denied("svc/exact-name.yml"));
        // A glob with a `/` is anchored at the root.
        assert!(!list.is_denied("svc/secrets/x.yml"));
        assert!(list.snapshot().is_denied_parts("x.pem", "a/x.pem"));
    }

    #[test]
    fn hub_globs_are_added_not_replaced() {
        let list = DenyList::default();
        assert!(!list.is_denied("svc/x.secret"));
        let applied = list.set_hub_globs(&["*.secret", "vault/*"]);
        assert_eq!(
            applied,
            Applied {
                changed: true,
                rejected: 0
            }
        );
        assert!(list.is_denied("svc/x.secret"));
        assert!(list.is_denied("vault/a.yml"));
        // The built-ins are still there.
        assert_defaults_hold(&list);
        assert_eq!(list.hub_globs(), ["*.secret", "vault/*"]);
    }

    #[test]
    fn default_deny_globs_cannot_be_removed() {
        let list = DenyList::default();
        list.set_hub_globs(&["*.secret"]);
        // Taking all of the hub's globs back leaves the built-ins.
        let applied = list.set_hub_globs(&[]);
        assert!(applied.changed);
        assert_defaults_hold(&list);
        assert!(!list.is_denied("svc/x.secret"));
        // Naming a default again, or something that looks like taking one away, changes nothing about the defaults.
        for attempt in [
            &["!*.pem"][..],
            &["*.pem"][..],
            &["^*.pem"][..],
            &["-*.pem"][..],
            &["[!.]*.pem"][..],
            &[""][..],
        ] {
            list.set_hub_globs(attempt);
            assert_defaults_hold(&list);
        }
    }

    #[test]
    fn a_hub_glob_that_does_not_compile_is_dropped_and_the_rest_are_used() {
        let list = DenyList::default();
        let applied = list.set_hub_globs(&["*.secret", "[unclosed", "{a,b", "ok/*", "a/**b[", "**/[z-a]"]);
        assert_eq!(applied.rejected, 4, "{applied:?}");
        assert!(applied.changed);
        assert!(list.is_denied("x.secret"));
        assert!(list.is_denied("ok/file"));
        assert_defaults_hold(&list);
        assert_eq!(list.rejected(), 4);
    }

    #[test]
    fn the_same_list_again_changes_nothing() {
        let list = DenyList::default();
        assert!(list.set_hub_globs(&["*.secret"]).changed);
        let again = list.set_hub_globs(&["*.secret"]);
        assert!(!again.changed);
        // Order and repeats do not matter.
        assert!(!list.set_hub_globs(&["*.secret", "*.secret"]).changed);
        assert!(list.set_hub_globs(&["*.other", "*.secret"]).changed);
        assert!(!list.set_hub_globs(&["*.secret", "*.other"]).changed);
        // Removing one is a change.
        assert!(list.set_hub_globs(&["*.secret"]).changed);
        assert!(!list.is_denied("x.other"));
    }

    #[test]
    fn too_many_and_too_long_hub_globs_are_cut_off() {
        let many: Vec<String> = (0..MAX_HUB_GLOBS + 10).map(|i| format!("*.ext{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let list = DenyList::new(&refs);
        assert_eq!(list.rejected(), 10);
        assert_eq!(list.hub_globs().len(), MAX_HUB_GLOBS);
        assert!(list.is_denied("x.ext0"));
        assert!(!list.is_denied(&format!("x.ext{}", MAX_HUB_GLOBS + 5)));
        assert_defaults_hold(&list);

        let long = "a".repeat(MAX_GLOB_BYTES + 1);
        let fits = "b".repeat(MAX_GLOB_BYTES);
        let list = DenyList::new(&[&long, &fits]);
        assert_eq!(list.rejected(), 1);
        assert!(list.is_denied(&fits));
    }

    #[test]
    fn many_globs_compile_in_chunks_and_all_match() {
        let globs: Vec<String> = (0..300).map(|i| format!("dir{i}/*.dat")).collect();
        let refs: Vec<&str> = globs.iter().map(String::as_str).collect();
        let list = DenyList::new(&refs);
        assert_eq!(list.rejected(), 0);
        assert!(list.is_denied("dir0/a.dat"));
        assert!(list.is_denied("dir299/a.dat"));
        assert!(!list.is_denied("dir300/a.dat"));
    }

    #[test]
    fn a_snapshot_does_not_move_when_the_list_does() {
        let list = DenyList::default();
        let before = list.snapshot();
        list.set_hub_globs(&["*.secret"]);
        let after = list.snapshot();
        assert!(!before.is_denied("x.secret"));
        assert!(after.is_denied("x.secret"));
        // A clone of the list is the same list.
        let clone = list.clone();
        clone.set_hub_globs(&["*.other"]);
        assert!(list.is_denied("x.other"));
        assert!(!list.is_denied("x.secret"));
    }

    fn entry(path: &str, content: &[u8], denied: bool) -> ScanEntry {
        ScanEntry {
            path: NfsPath::parse(path).unwrap(),
            hash: ContentHash::from_bytes([7; 32]),
            size: content.len() as u64,
            mtime: Timestamp::from_unix_millis(1),
            observed_at: Timestamp::from_unix_millis(2),
            denied,
            bytes: (!denied).then(|| Bytes::copy_from_slice(content)),
        }
    }

    fn delta(entries: Vec<ScanEntry>) -> ScanDelta {
        ScanDelta {
            seq: 1,
            base_root: None,
            new_root: ContentHash::from_bytes([1; 32]),
            entries,
            removed: Vec::new(),
            skipped: Vec::new(),
            during_job: None,
            more: false,
            part: 0,
            gap: None,
        }
    }

    #[test]
    fn scrub_strips_bytes_and_keeps_name_size_and_hash() {
        let list = DenyList::new(&["*.secret"]);
        let mut message = delta(vec![
            entry("svc/a.yml", b"plain", false),
            entry("svc/tls.pem", b"-----BEGIN PRIVATE KEY-----", false),
            entry("svc/x.secret", b"hub-denied", false),
            entry("svc/already.key", b"", true),
        ]);
        let changed = list.scrub(&mut message);
        assert_eq!(changed, 2);
        let [plain, pem, secret, already] = &message.entries[..] else {
            panic!("four entries");
        };
        assert!(!plain.denied && plain.bytes.as_deref() == Some(&b"plain"[..]));
        for denied in [pem, secret, already] {
            assert!(denied.denied && denied.bytes.is_none(), "{:?}", denied.path);
        }
        assert_eq!(pem.size, 27, "the size stays");
        assert_eq!(pem.hash, ContentHash::from_bytes([7; 32]), "the hash stays");
        // Scrubbing twice changes nothing more.
        assert_eq!(list.scrub(&mut message), 0);
    }

    #[test]
    fn debug_shows_counts_and_no_globs() {
        let list = DenyList::new(&["*.very-secret-name"]);
        let shown = format!("{list:?} {:?}", list.snapshot());
        assert!(!shown.contains("very-secret-name"), "{shown}");
    }

    proptest! {
        #[test]
        fn any_hub_list_leaves_the_defaults_in_force(
            hub in proptest::collection::vec(".{0,40}", 0..24),
        ) {
            let refs: Vec<&str> = hub.iter().map(String::as_str).collect();
            let list = DenyList::new(&refs);
            for sample in DENIED_SAMPLES {
                prop_assert!(list.is_denied(sample), "{sample}");
            }
            list.set_hub_globs(&refs);
            list.set_hub_globs(&[]);
            for sample in DENIED_SAMPLES {
                prop_assert!(list.is_denied(sample), "{sample}");
            }
            prop_assert!(list.rejected() <= refs.len());
        }

        #[test]
        fn matching_never_panics_on_any_path(path in ".{0,200}") {
            let list = DenyList::new(&["*.x", "a/**/b", "[ab]*"]);
            let _ = list.is_denied(&path);
        }
    }
}
