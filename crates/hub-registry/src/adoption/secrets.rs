//! The secret scan of the adoption pass (T5, step 4, S10, S13, Q20, D79): where in the repositories and on NFS a value that looks like a
//! secret may be.
//!
//! The scan runs `engine::secret_scan` (value-based: a private key block, a known token, a JWT, a long random-looking string, never a
//! name) over the text of
//!
//! - the **head of the base repository's default branch** and the **heads of the tenant branches the admin chose** (the commits fixed when
//!   the branches were chosen), because those are what everyone reads from Git;
//! - the **NFS content** of the swimlane (the blobs the registry holds for its current files).
//!
//! A denied file (D79: `*.pem`, `*.key`, `*private*`, ...) is not read, anywhere: it is not scanned, and it is already shown as denied. A
//! binary file is not text. A file that cannot be checked (the scan stopped at its limits, the text cannot be decoded, a blob that is
//! not stored) is listed with the rule [`UNSCANNED_RULE`]: a place that was not looked at is never reported as clean.
//!
//! **What is stored is where, never what** (S13): [`SecretHit`] is a file, a line, a length and a rule. The engine's match holds no value,
//! this module never holds one past the scan of the file it came from, and nothing here logs, returns or writes a value or a hash of
//! content: the log lines carry ids and counts.
//!
//! The same content scanned once: Git files that equal NFS files (the usual case) share one scan, and the hits are listed under every
//! path that has the content. Content is read in batches bounded by count and by bytes (`adoption_scan_batch_*`), so memory never holds
//! more than a batch, and the CPU work of a batch runs on the registry's rayon pool inside `spawn_blocking` (rule 4).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use bytes::Bytes;
use domain::{CommitId, ContentHash, FileClass, NfsPath, PathMappingRule, RepoKind, RepoPath, TenantId};
use engine::{SecretMatch, classify, decode_text, secret_scan};
use rayon::prelude::*;
use serde_json::{Value, json};
use tracing::info;

use super::resolutions::NfsFile;
use super::store::{AdoptionStep, Row};
use super::{AdoptionError, AdoptionView, Inner};
use crate::db::ts;
use crate::error::RegistryError;
use crate::pipeline::blocking;

/// The rule of a place the scan could not check.
pub const UNSCANNED_RULE: &str = "unscanned";
/// The rule of the last entry of a scan that found more places than it keeps: `length` is how many are not listed.
pub const TRUNCATED_RULE: &str = "truncated";

/// Where a secret may be. A place, a size and a kind, never a value (S13).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretHit {
    /// The path in the repository (`config/...`, `data/config/...`) or on NFS.
    pub file: String,
    /// 1-based; 0 for an entry that is about the whole file ([`UNSCANNED_RULE`]).
    pub line: u32,
    /// In characters.
    pub length: u32,
    /// The engine's rule name (`github_token`, `high_entropy`, ...), or [`UNSCANNED_RULE`] or [`TRUNCATED_RULE`].
    pub rule: String,
}

impl SecretHit {
    pub(crate) fn to_json(&self) -> Value {
        json!({"file": self.file, "line": self.line, "length": self.length, "rule": self.rule})
    }

    pub(crate) fn list_from_json(value: &Value) -> Result<Vec<Self>, RegistryError> {
        let Some(list) = value.as_array() else {
            return Err(RegistryError::Corrupt);
        };
        list.iter()
            .map(|h| {
                let number = |key: &str| {
                    h.get(key)
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or(RegistryError::Corrupt)
                };
                Ok(Self {
                    file: h
                        .get("file")
                        .and_then(Value::as_str)
                        .ok_or(RegistryError::Corrupt)?
                        .to_owned(),
                    line: number("line")?,
                    length: number("length")?,
                    rule: h
                        .get("rule")
                        .and_then(Value::as_str)
                        .ok_or(RegistryError::Corrupt)?
                        .to_owned(),
                })
            })
            .collect()
    }
}

/// Where the bytes of a content come from.
#[derive(Debug, Clone)]
struct Source {
    size: u64,
    /// The registry holds the content as a blob (an NFS file has it).
    blob: bool,
    /// A Git file that has it.
    git: Option<(RepoKind, CommitId, RepoPath)>,
}

/// A place to report, and the content it holds.
#[derive(Debug, Clone)]
struct Location {
    file: String,
    hash: ContentHash,
}

/// What scanning one content gave.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Hits(Vec<(u32, u32, &'static str)>),
    Unscanned,
}

/// Scan one content. A text that cannot be decoded or that the scan cannot finish is `Unscanned`, never "clean".
fn scan_one(data: &[u8], limits: &engine::Limits, config: &engine::ScanConfig) -> Outcome {
    let Ok(decoded) = decode_text(data, limits) else {
        return Outcome::Unscanned;
    };
    match secret_scan(&decoded.text, config, limits) {
        Ok(matches) => Outcome::Hits(matches.iter().map(summary).collect()),
        Err(_) => Outcome::Unscanned,
    }
}

fn summary(m: &SecretMatch) -> (u32, u32, &'static str) {
    (m.line, m.length, m.rule.name())
}

/// The batches of contents to read together: at most `files` of them and `bytes` bytes, in hash order. A content bigger than the budget is
/// a batch of its own.
fn batches(unique: &BTreeMap<ContentHash, Source>, files: usize, bytes: u64) -> Vec<Vec<ContentHash>> {
    let mut out: Vec<Vec<ContentHash>> = Vec::new();
    let (mut current, mut used): (Vec<ContentHash>, u64) = (Vec::new(), 0);
    for (hash, source) in unique {
        if !current.is_empty() && (current.len() >= files || used.saturating_add(source.size) > bytes) {
            out.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(*hash);
        used = used.saturating_add(source.size);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The hits of every location, in order, within the cap; with the entry that says how many were left out.
fn collect_hits(
    locations: &[Location],
    outcomes: &HashMap<ContentHash, Outcome>,
    cap: usize,
) -> Vec<SecretHit> {
    let mut all: BTreeSet<SecretHit> = BTreeSet::new();
    for location in locations {
        match outcomes.get(&location.hash) {
            Some(Outcome::Hits(hits)) => {
                for (line, length, rule) in hits {
                    all.insert(SecretHit {
                        file: location.file.clone(),
                        line: *line,
                        length: *length,
                        rule: (*rule).to_owned(),
                    });
                }
            }
            // A content that was not scanned, or that could not be read at all, is a place nobody has looked at.
            Some(Outcome::Unscanned) | None => {
                all.insert(SecretHit {
                    file: location.file.clone(),
                    line: 0,
                    length: 0,
                    rule: UNSCANNED_RULE.to_owned(),
                });
            }
        }
    }
    let total = all.len();
    let mut kept: Vec<SecretHit> = all.into_iter().take(cap).collect();
    if total > cap {
        kept.push(SecretHit {
            file: "(truncated)".to_owned(),
            line: 0,
            length: u32::try_from(total - cap).unwrap_or(u32::MAX),
            rule: TRUNCATED_RULE.to_owned(),
        });
    }
    kept
}

/// The files of a tree index that the scan reads: under `root`, text by the engine's classification of the NFS path they map to, and not denied.
fn git_locations(
    index: &domain::TreeIndex,
    repo: RepoKind,
    commit: &CommitId,
    map: &dyn Fn(&RepoPath) -> Option<NfsPath>,
    deny: &engine::DenyMatcher,
    locations: &mut Vec<Location>,
    unique: &mut BTreeMap<ContentHash, Source>,
) {
    for entry in index.entries() {
        let Some(nfs) = map(&entry.path) else { continue };
        if !matches!(
            classify(&nfs, None, deny),
            FileClass::Structured | FileClass::Text
        ) {
            continue;
        }
        locations.push(Location {
            file: entry.path.as_str().to_owned(),
            hash: entry.sha256,
        });
        let source = unique.entry(entry.sha256).or_insert(Source {
            size: entry.size,
            blob: false,
            git: None,
        });
        if source.git.is_none() {
            source.git = Some((repo, commit.clone(), entry.path.clone()));
        }
    }
}

/// The places to scan: the NFS files that are text and not denied, and the base head and chosen tenant commits' files.
#[allow(clippy::type_complexity)]
fn places(
    nfs: &[NfsFile],
    git: &dyn ports::GitReader,
    rule: &PathMappingRule,
    deny: &engine::DenyMatcher,
    tenants: &BTreeMap<TenantId, CommitId>,
) -> Result<(Vec<Location>, BTreeMap<ContentHash, Source>), RegistryError> {
    let mut locations: Vec<Location> = Vec::new();
    let mut unique: BTreeMap<ContentHash, Source> = BTreeMap::new();
    for file in nfs {
        if file.denied || !matches!(file.class, FileClass::Structured | FileClass::Text) {
            continue;
        }
        locations.push(Location {
            file: file.path.as_str().to_owned(),
            hash: file.raw,
        });
        let source = unique.entry(file.raw).or_insert(Source {
            size: file.size.unwrap_or(0),
            blob: true,
            git: None,
        });
        source.blob = true;
    }
    let branches = git.branches(RepoKind::Base)?;
    if let Some(default) = branches
        .iter()
        .find(|b| b.kind == domain::BranchKind::Default)
        .or_else(|| branches.first())
    {
        let index = git.tree_index(RepoKind::Base, &default.head)?;
        git_locations(
            &index,
            RepoKind::Base,
            &default.head,
            &|p| rule.base_to_nfs(p).ok(),
            deny,
            &mut locations,
            &mut unique,
        );
    }
    for (tenant, commit) in tenants {
        let index = git.tree_index(RepoKind::Tenant, commit)?;
        git_locations(
            &index,
            RepoKind::Tenant,
            commit,
            &|p| rule.tenant_to_nfs(p, tenant).ok(),
            deny,
            &mut locations,
            &mut unique,
        );
    }
    Ok((locations, unique))
}

impl Inner {
    /// Scan, and store where the hits are. See the module's documentation.
    pub(crate) async fn scan_secrets(&self, id: i64) -> Result<AdoptionView, AdoptionError> {
        let row = self.working_row(id).await?;
        if row.step < AdoptionStep::Files {
            return Err(AdoptionError::WrongStep);
        }
        let hits = match tokio::time::timeout(self.config.adoption_scan_timeout, self.scan(&row)).await {
            Ok(hits) => hits?,
            Err(_) => return Err(RegistryError::Timeout.into()),
        };
        let stored = Value::Array(hits.iter().map(SecretHit::to_json).collect());
        let now = self.clock.now().unix_millis();
        // The step moves on from the secret scan to the confirmation, and a scan made earlier leaves the step alone.
        let sql = format!(
            "UPDATE adoptions SET secret_scan = $2::jsonb, updated_at = {}, \
             step = CASE WHEN step = 'secrets' THEN 'confirm' ELSE step END \
             WHERE id = $1 AND status = 'in_progress'",
            ts("$3")
        );
        let done = self
            .db
            .run(
                "adoption_scan_store",
                sqlx::query(&sql)
                    .bind(id)
                    .bind(stored.to_string())
                    .bind(now)
                    .execute(self.db.pool()),
            )
            .await?;
        if done.rows_affected() == 0 {
            return Err(AdoptionError::NotOpen);
        }
        self.metrics.adoption_secrets(hits.len() as u64);
        info!(adoption = id, swimlane = %row.swimlane, places = hits.len(), "adoption secret scan finished");
        self.view(id).await
    }

    /// The places that may hold a secret.
    async fn scan(&self, row: &Row) -> Result<Vec<SecretHit>, RegistryError> {
        let tenants = row.commits()?;
        let rule = self.mapping_rule().await?;
        let nfs = self.nfs_files(&row.swimlane).await?;
        let (git, deny) = (
            std::sync::Arc::clone(&self.git),
            std::sync::Arc::clone(&self.deny),
        );
        let timeout = self.config.git_timeout.max(Duration::from_secs(60));
        let (locations, unique) =
            blocking(timeout, move || places(&nfs, &*git, &rule, &deny, &tenants)).await?;

        let mut outcomes: HashMap<ContentHash, Outcome> = HashMap::with_capacity(unique.len());
        for batch in batches(
            &unique,
            self.config.adoption_scan_batch_files,
            self.config.adoption_scan_batch_bytes,
        ) {
            let contents = self.contents(&batch, &unique).await?;
            let (pool, limits, scan) = (
                std::sync::Arc::clone(&self.pool),
                self.limits.clone(),
                self.scan.clone(),
            );
            let scanned: Vec<(ContentHash, Outcome)> = blocking(timeout, move || {
                Ok(pool.install(|| {
                    contents
                        .par_iter()
                        .map(|(hash, bytes)| (*hash, scan_one(bytes, &limits, &scan)))
                        .collect()
                }))
            })
            .await?;
            outcomes.extend(scanned);
            // A content that could not be read is simply not in `outcomes`: its places are reported as unscanned.
        }
        Ok(collect_hits(
            &locations,
            &outcomes,
            self.config.adoption_secret_hits,
        ))
    }

    /// The bytes of a batch: from the blob store where the registry holds the content, from Git for the rest.
    async fn contents(
        &self,
        batch: &[ContentHash],
        unique: &BTreeMap<ContentHash, Source>,
    ) -> Result<Vec<(ContentHash, Bytes)>, RegistryError> {
        let wanted: Vec<ContentHash> = batch
            .iter()
            .filter(|h| unique.get(h).is_some_and(|s| s.blob))
            .copied()
            .collect();
        let mut found: HashMap<ContentHash, Bytes> = HashMap::new();
        if !wanted.is_empty() {
            let stored = tokio::time::timeout(self.config.blob_timeout, self.blobs.get_many(&wanted))
                .await
                .map_err(|_| RegistryError::Timeout)??;
            found.extend(stored);
        }
        // A Git file bigger than `max_file_bytes` is never read (the port has no read with a limit): no NFS file can be that big, so it
        // is not a file this pass compares, and its places are reported as unscanned.
        let largest = self.config.max_file_bytes;
        let from_git: Vec<(ContentHash, RepoKind, CommitId, RepoPath)> = batch
            .iter()
            .filter(|h| !found.contains_key(h))
            .filter_map(|h| {
                let source = unique.get(h)?;
                if source.size > largest {
                    return None;
                }
                let (repo, commit, path) = source.git.clone()?;
                Some((*h, repo, commit, path))
            })
            .collect();
        if !from_git.is_empty() {
            let git = std::sync::Arc::clone(&self.git);
            let read = blocking(self.config.git_timeout.max(Duration::from_secs(60)), move || {
                let mut out: Vec<(ContentHash, Bytes)> = Vec::with_capacity(from_git.len());
                for (hash, repo, commit, path) in from_git {
                    if let Some(bytes) = git.read(repo, &commit, &path)? {
                        out.push((hash, bytes));
                    }
                }
                Ok(out)
            })
            .await?;
            found.extend(read);
        }
        Ok(batch
            .iter()
            .filter_map(|h| found.remove(h).map(|bytes| (*h, bytes)))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use ports::content_hash;

    use super::*;

    fn h(s: &str) -> ContentHash {
        content_hash(s.as_bytes())
    }

    fn scan(text: &str) -> Outcome {
        scan_one(
            text.as_bytes(),
            &engine::Limits::default(),
            &engine::ScanConfig::default(),
        )
    }

    const TOKEN: &str = "ghp_aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bD1fH3jL5";

    #[test]
    fn a_hit_is_a_line_a_length_and_a_rule() {
        let Outcome::Hits(hits) = scan(&format!("name: x\ntoken: {TOKEN}\n")) else {
            panic!("the text can be scanned")
        };
        assert_eq!(
            hits,
            vec![(2, u32::try_from(TOKEN.len()).unwrap(), "github_token")]
        );
        assert_eq!(scan("a: 1\nb: two\n"), Outcome::Hits(Vec::new()));
    }

    #[test]
    fn a_text_that_cannot_be_checked_is_unscanned_and_not_clean() {
        // A UTF-8 byte order mark followed by bytes that are not UTF-8: the declaration is not honoured by the body, so it is no text.
        assert_eq!(
            scan_one(
                b"\xEF\xBB\xBF\xFF\xFE",
                &engine::Limits::default(),
                &engine::ScanConfig::default()
            ),
            Outcome::Unscanned
        );
        // Over the engine's input limit.
        let limits = engine::Limits {
            input_bytes: 8,
            ..engine::Limits::default()
        };
        assert_eq!(
            scan_one(b"a: 1234567890", &limits, &engine::ScanConfig::default()),
            Outcome::Unscanned
        );
    }

    #[test]
    fn an_entry_serialises_to_the_four_keys_of_the_table_and_reads_back() {
        let hit = SecretHit {
            file: "config/a.yml".to_owned(),
            line: 3,
            length: 40,
            rule: "github_token".to_owned(),
        };
        let json = hit.to_json();
        let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["file", "length", "line", "rule"],
            "file, line, length and rule only (S13)"
        );
        assert_eq!(
            SecretHit::list_from_json(&Value::Array(vec![json])).unwrap(),
            vec![hit]
        );
        assert_eq!(SecretHit::list_from_json(&json!({})), Err(RegistryError::Corrupt));
        assert_eq!(
            SecretHit::list_from_json(&json!([{"file": "a", "line": -1, "length": 1, "rule": "r"}])),
            Err(RegistryError::Corrupt)
        );
    }

    #[test]
    fn contents_are_batched_by_count_and_by_bytes_and_none_is_lost() {
        let mut unique = BTreeMap::new();
        for (i, size) in [10_u64, 10, 10, 500, 10, 10].iter().enumerate() {
            unique.insert(
                h(&format!("c{i}")),
                Source {
                    size: *size,
                    blob: true,
                    git: None,
                },
            );
        }
        let all: BTreeSet<ContentHash> = unique.keys().copied().collect();
        let by_count = batches(&unique, 2, u64::MAX);
        assert!(by_count.iter().all(|b| b.len() <= 2));
        assert_eq!(by_count.concat().into_iter().collect::<BTreeSet<_>>(), all);
        let by_bytes = batches(&unique, 100, 100);
        // The 500-byte content is over the budget alone, so it is a batch of its own.
        assert!(by_bytes.iter().any(|b| b.len() == 1 && unique[&b[0]].size == 500));
        assert!(
            by_bytes
                .iter()
                .all(|b| b.len() == 1 || b.iter().map(|x| unique[x].size).sum::<u64>() <= 100)
        );
        assert_eq!(by_bytes.concat().into_iter().collect::<BTreeSet<_>>(), all);
        assert!(batches(&BTreeMap::new(), 2, 100).is_empty());
    }

    #[test]
    fn the_hits_are_listed_under_every_path_that_has_the_content_and_unread_content_is_unscanned() {
        let locations = vec![
            Location {
                file: "app/a.yml".to_owned(),
                hash: h("a"),
            },
            Location {
                file: "config/app/a.yml".to_owned(),
                hash: h("a"),
            },
            Location {
                file: "app/b.yml".to_owned(),
                hash: h("b"),
            },
            Location {
                file: "app/gone.yml".to_owned(),
                hash: h("gone"),
            },
            Location {
                file: "app/big.yml".to_owned(),
                hash: h("big"),
            },
        ];
        let outcomes = HashMap::from([
            (h("a"), Outcome::Hits(vec![(2, 40, "github_token")])),
            (h("b"), Outcome::Hits(Vec::new())),
            (h("big"), Outcome::Unscanned),
        ]);
        let hits = collect_hits(&locations, &outcomes, 100);
        let listed: Vec<(&str, u32, &str)> = hits
            .iter()
            .map(|x| (x.file.as_str(), x.line, x.rule.as_str()))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("app/a.yml", 2, "github_token"),
                ("app/big.yml", 0, "unscanned"),
                ("app/gone.yml", 0, "unscanned"),
                ("config/app/a.yml", 2, "github_token"),
            ],
            "sorted; a clean file is not listed; a place that was not checked is"
        );
    }

    #[test]
    fn a_scan_with_more_places_than_it_keeps_says_how_many_it_left_out() {
        let locations: Vec<Location> = (0..10)
            .map(|i| Location {
                file: format!("f{i}.yml"),
                hash: h("x"),
            })
            .collect();
        let outcomes = HashMap::from([(h("x"), Outcome::Hits(vec![(1, 32, "high_entropy")]))]);
        let hits = collect_hits(&locations, &outcomes, 4);
        assert_eq!(hits.len(), 5);
        let last = hits.last().unwrap();
        assert_eq!((last.rule.as_str(), last.length), (TRUNCATED_RULE, 6));
        assert!(hits[..4].iter().all(|x| x.rule == "high_entropy"));
        // Exactly at the cap nothing is cut.
        assert_eq!(collect_hits(&locations, &outcomes, 10).len(), 10);
    }

    mod property {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            /// With room for everything, a place is listed exactly when the scan found something there or could not check it: a clean
            /// file is never listed, and a place that was not read is never taken for clean. The list is sorted and has no repeats.
            #[test]
            fn a_place_is_listed_exactly_when_it_may_hold_a_secret_or_was_not_checked(
                places in prop::collection::vec((0_usize..6, 0_usize..5), 0..30),
                outcomes in prop::collection::vec(0_u8..4, 5),
            ) {
                let locations: Vec<Location> = places
                    .iter()
                    .map(|(file, content)| Location { file: format!("f{file}.yml"), hash: h(&format!("c{content}")) })
                    .collect();
                let by_content: HashMap<ContentHash, Outcome> = outcomes
                    .iter()
                    .enumerate()
                    // Outcome 3 is a content that was never read: it has no entry at all.
                    .filter_map(|(i, o)| {
                        let outcome = match o {
                            0 => Outcome::Hits(Vec::new()),
                            1 => Outcome::Hits(vec![(u32::try_from(i).unwrap() + 1, 32, "high_entropy")]),
                            2 => Outcome::Unscanned,
                            _ => return None,
                        };
                        Some((h(&format!("c{i}")), outcome))
                    })
                    .collect();
                let hits = collect_hits(&locations, &by_content, usize::MAX / 2);
                prop_assert!(hits.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
                for location in &locations {
                    let listed = hits.iter().any(|x| x.file == location.file && x.rule != TRUNCATED_RULE);
                    let mut expected = false;
                    // The file is listed if any location of that name has a content with hits or without a verdict.
                    for other in locations.iter().filter(|o| o.file == location.file) {
                        expected |= !matches!(by_content.get(&other.hash), Some(Outcome::Hits(h)) if h.is_empty());
                    }
                    prop_assert_eq!(listed, expected, "{}", location.file);
                }
                prop_assert!(hits.iter().all(|x| x.rule != TRUNCATED_RULE));
            }

            /// With less room, at most the cap is kept, and one entry says how many were left out.
            #[test]
            fn a_list_over_the_cap_is_cut_and_counted(count in 1_usize..40, cap in 1_usize..20) {
                let locations: Vec<Location> = (0..count).map(|i| Location { file: format!("f{i:03}.yml"), hash: h("x") }).collect();
                let outcomes = HashMap::from([(h("x"), Outcome::Hits(vec![(1, 32, "high_entropy")]))]);
                let hits = collect_hits(&locations, &outcomes, cap);
                if count <= cap {
                    prop_assert_eq!(hits.len(), count);
                } else {
                    prop_assert_eq!(hits.len(), cap + 1);
                    let last = hits.last().unwrap();
                    prop_assert_eq!(last.rule.as_str(), TRUNCATED_RULE);
                    prop_assert_eq!(usize::try_from(last.length).unwrap(), count - cap);
                }
            }
        }
    }
}
