//! Compare one file between two references (T6, P7: 100 ms for a 6,000-line file, D65, D76, A28, A30).
//!
//! # The sides
//!
//! `nfs:<swimlane>` and `baseline:<swimlane>` are the file the swimlane's standing tenant reads (A30): its tenant copy when it has one,
//! else the base file. `effective:<swimlane>` is the engine's effective config of the file for that tenant (a property source has
//! settings and no text; a resource is the file the config-server would choose, channel folder first, unrendered). `git:base@<ref>`
//! and `git:tenant@<ref>` are the repository file for the logical file at a commit, branch or tag. A file that does not exist on a side
//! is an *absent* side, and a file that exists on neither is `NotFound`.
//!
//! # The answer
//!
//! - **Denied** (D79) on either side: nothing is compared but the hashes and sizes; no text and no settings are read.
//! - **Binary**: hash and size (`binary_compare`).
//! - **Text and structured**: the line diff in hunks (`text_diff`, with line endings normalised first, a difference in line endings
//!   alone flagged `eol_only`), and for a structured file the settings that differ (`semantic_diff`) from the setting rows of both
//!   sides, which hold flagged values already redacted (A28). When a side has a flagged row the real bytes of both sides are parsed
//!   instead and the flagged values are masked on the way out, because two redaction markers of one length compare equal and a
//!   changed secret must not look unchanged. The hunks are the raw text (A28: unmasked, a recorded residual risk).
//!
//! # Caching and the entity tag
//!
//! The diff is cached by the hashes of its two sides, so it is never stale. The tag is derived from the two hashes and the references
//! (`etag`), and a request that already has it is answered before any diff is made. The response does not carry the tenants in its
//! body (the contract has no field); the REST edge names them in a header.

use std::sync::Arc;

use bytes::Bytes;
use domain::{
    BinaryCompare, ChannelName, CompareRef, Comparison, ContentHash, FileClass, FileRole, GitRef,
    LogicalFile, NfsPath, RepoKind, RepoPath, SettingChange, SettingPath, SettingValue, SwimlaneId, TenantId,
};
use engine::{
    BinaryFacts, FlatSetting, Limits, Syntax, binary_compare, classify, decode_text, flatten, parse,
    resolve_resource, semantic_diff, text_diff,
};
use ports::ReadError;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{BTreeSet, HashMap};
use tracing::warn;

use super::api::{Named, Tagged};
use super::cache::{BlobRows, DiffEntry, EffectiveEntry};
use super::effective::{Group, LayerRow, layering};
use super::etag;
use super::rows;
use super::service::Inner;
use crate::metrics::CacheName;

const BASELINE_BLOB_SQL: &str = "SELECT blob_hash::bytea FROM baselines WHERE swimlane_id = $1 AND path = $2";

/// What a side has as text.
enum Text {
    /// The file does not exist on this side.
    Absent,
    Bytes(Bytes),
    /// The side has settings and no text (a merged property source).
    None,
}

/// One side of a comparison, loaded.
struct Side {
    /// `nfs`, `baseline`, `effective` or `git`: part of the key of the cache, because the same bytes mean different things.
    kind: &'static str,
    class: FileClass,
    withheld: bool,
    /// The identity of the content: the hash of the bytes, or the effective hash of a merged property source. `None` when absent.
    hash: Option<ContentHash>,
    size: Option<u64>,
    text: Text,
    rows: Option<Arc<BlobRows>>,
    effective: Option<Arc<EffectiveEntry>>,
    syntax: Option<Syntax>,
    tenant: Option<(SwimlaneId, TenantId)>,
}

impl Side {
    fn absent(kind: &'static str) -> Self {
        Self {
            kind,
            class: FileClass::Text,
            withheld: false,
            hash: None,
            size: None,
            text: Text::Absent,
            rows: None,
            effective: None,
            syntax: None,
            tenant: None,
        }
    }

    fn is_absent(&self) -> bool {
        self.hash.is_none() && matches!(self.text, Text::Absent)
    }

    fn identity(&self) -> String {
        self.hash.map_or_else(|| "-".to_owned(), |h| h.to_string())
    }
}

fn pair_class(l: &Side, r: &Side) -> FileClass {
    let present: Vec<&Side> = [l, r].into_iter().filter(|s| !s.is_absent()).collect();
    if present.iter().any(|s| s.withheld || s.class == FileClass::Denied) {
        FileClass::Denied
    } else if present.iter().any(|s| s.class == FileClass::Binary) {
        FileClass::Binary
    } else if present.iter().any(|s| s.class == FileClass::Structured) {
        FileClass::Structured
    } else {
        present.first().map_or(FileClass::Text, |s| s.class)
    }
}

fn facts(s: &Side) -> Option<BinaryFacts> {
    Some(BinaryFacts {
        hash: s.hash?,
        size: s.size.unwrap_or(0),
    })
}

/// The settings a side shows, as the engine's flat settings.
fn settings_of(s: &Side) -> Vec<FlatSetting> {
    if let Some(e) = &s.effective {
        return e
            .effective
            .settings
            .iter()
            .map(|v| FlatSetting {
                path: v.path.clone(),
                value: v.value.clone(),
                lines: v.origin.lines,
                via_alias: false,
            })
            .collect();
    }
    s.rows.as_ref().map(|r| r.settings.clone()).unwrap_or_default()
}

/// The real settings of a side's bytes, parsed. `None` when they do not parse.
fn real_settings(bytes: &Bytes, syntax: Syntax, limits: &Limits) -> Option<Vec<FlatSetting>> {
    let decoded = decode_text(bytes, limits).ok()?;
    let parsed = parse(&decoded.text, syntax, limits).ok()?;
    Some(flatten(&parsed, limits).ok()?.settings)
}

/// The redaction markers of the flagged rows of a side, by setting path.
fn markers(s: &Side) -> HashMap<&SettingPath, &SettingValue> {
    let Some(rows) = &s.rows else {
        return HashMap::new();
    };
    rows.settings
        .iter()
        .zip(&rows.secret)
        .filter(|(_, secret)| **secret)
        .map(|(setting, _)| (&setting.path, &setting.value))
        .collect()
}

fn mask(
    changes: &mut [SettingChange],
    left: &HashMap<&SettingPath, &SettingValue>,
    right: &HashMap<&SettingPath, &SettingValue>,
) {
    for change in changes {
        if let (Some(v), Some(marker)) = (change.left.as_mut(), left.get(&change.path)) {
            *v = (*marker).clone();
        }
        if let (Some(v), Some(marker)) = (change.right.as_mut(), right.get(&change.path)) {
            *v = (*marker).clone();
        }
    }
}

/// What a comparison says about the two contents, whichever references named them: the cached part of a [`Comparison`].
#[derive(Debug, Clone)]
pub struct DiffBody {
    pub class: FileClass,
    pub identical: bool,
    pub eol_only: bool,
    pub hunks: Vec<domain::DiffHunk>,
    pub setting_changes: Vec<SettingChange>,
    pub binary: Option<BinaryCompare>,
}

/// The comparison of two loaded sides. Pure and CPU-bound: runs on the blocking pool.
fn diff(left: &Side, right: &Side, limits: &Limits) -> DiffBody {
    let class = pair_class(left, right);
    let blank = |class, identical, binary| DiffBody {
        class,
        identical,
        eol_only: false,
        hunks: Vec::new(),
        setting_changes: Vec::new(),
        binary,
    };
    let by_hash = || {
        let d = binary_compare(facts(left), facts(right));
        blank(class, d.identical, Some(d.compare))
    };
    if class == FileClass::Denied {
        // Nothing of a denied file is read: the answer is the hashes and the sizes.
        let d = binary_compare(facts(left), facts(right));
        return blank(FileClass::Denied, d.identical, Some(d.compare));
    }
    if class == FileClass::Binary {
        return by_hash();
    }

    // Text. A side whose bytes do not decode is compared by hash, as a binary file is.
    let decode = |t: &Text| -> Result<Option<String>, ()> {
        match t {
            Text::Bytes(b) => decode_text(b, limits)
                .map(|d| Some(d.text.into_owned()))
                .map_err(|_| ()),
            Text::Absent => Ok(Some(String::new())),
            Text::None => Ok(None),
        }
    };
    let (Ok(lt), Ok(rt)) = (decode(&left.text), decode(&right.text)) else {
        return by_hash();
    };
    let mut out = blank(class, false, None);
    let mut identical_text = None;
    if let (Some(l), Some(r)) = (&lt, &rt) {
        let d = text_diff(l, r, limits);
        out.eol_only = d.eol_only;
        identical_text = Some(d.identical);
        out.hunks = d.hunks;
    }

    if class == FileClass::Structured {
        let unparseable = [left, right]
            .iter()
            .any(|s| s.rows.as_ref().is_some_and(|r| r.unparseable));
        if !unparseable {
            let secret = [left, right]
                .iter()
                .any(|s| s.rows.as_ref().is_some_and(|r| r.any_secret()));
            let real = if secret {
                // The real values of both sides, so that a changed secret is a change; they are masked on the way out.
                match (&left.text, left.syntax, &right.text, right.syntax) {
                    (Text::Bytes(lb), Some(ls), Text::Bytes(rb), Some(rs)) => {
                        real_settings(lb, ls, limits).zip(real_settings(rb, rs, limits))
                    }
                    (Text::Absent, _, Text::Bytes(rb), Some(rs)) => {
                        real_settings(rb, rs, limits).map(|r| (Vec::new(), r))
                    }
                    (Text::Bytes(lb), Some(ls), Text::Absent, _) => {
                        real_settings(lb, ls, limits).map(|l| (l, Vec::new()))
                    }
                    _ => None,
                }
            } else {
                None
            };
            out.setting_changes = if let Some((l, r)) = real {
                let mut changes = semantic_diff(&l, &r);
                mask(&mut changes, &markers(left), &markers(right));
                changes
            } else {
                semantic_diff(&settings_of(left), &settings_of(right))
            };
        }
    }
    out.identical =
        identical_text.unwrap_or_else(|| out.setting_changes.is_empty() && left.hash == right.hash);
    out
}

fn weight_of(c: &DiffBody) -> u32 {
    let bytes: usize = c
        .hunks
        .iter()
        .map(|h| {
            96 + h
                .lines
                .iter()
                .map(|l| 64 + l.text.len() + l.inline_changes.len() * 8)
                .sum::<usize>()
        })
        .sum::<usize>()
        + c.setting_changes.len() * 400
        + 512;
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

impl Inner {
    /// The file a swimlane's standing tenant reads: its tenant copy, else the base file. `None` when the swimlane has neither.
    async fn chosen_row(
        &self,
        group: &Group,
        tenant: &TenantId,
        file: &LogicalFile,
        channel: Option<&ChannelName>,
    ) -> Result<Option<LayerRow>, ReadError> {
        let l = layering(group, file, tenant);
        if l.role == FileRole::Resource && channel.is_some() {
            // A resource asked for in a channel: the channel folder's copy first, by the engine's rule.
            let mut rows: Vec<LayerRow> = group.files.clone();
            if let Some(ch) = channel {
                let folder = file.as_path().parent();
                let name = file.as_path().file_name();
                let dir = if folder.is_root() {
                    ch.to_string()
                } else {
                    format!("{folder}/{ch}")
                };
                let (stem, ext) = name.rsplit_once('.').map_or((name, None), |(s, e)| (s, Some(e)));
                let tenant_name =
                    ext.map_or_else(|| format!("{stem}-{tenant}"), |e| format!("{stem}-{tenant}.{e}"));
                rows.extend(
                    self.layer_rows_by_path(
                        &group.swimlane,
                        &[format!("{dir}/{tenant_name}"), format!("{dir}/{name}")],
                    )
                    .await?,
                );
            }
            let present: BTreeSet<NfsPath> = rows.iter().map(|r| r.path.clone()).collect();
            return Ok(resolve_resource(file, Some(tenant), channel, &present)
                .and_then(|c| rows.into_iter().find(|r| r.path == c.path)));
        }
        Ok(l.tenant.or(l.base))
    }

    /// One side, loaded.
    async fn load_side(
        &self,
        reference: &CompareRef,
        file: &LogicalFile,
        channel: Option<&ChannelName>,
    ) -> Result<Side, ReadError> {
        match reference {
            CompareRef::Nfs(lane) | CompareRef::Baseline(lane) | CompareRef::Effective(lane) => {
                let groups = self.groups_of(std::slice::from_ref(lane), file).await?;
                let group = groups.get(lane).ok_or(ReadError::NotFound)?;
                let kind = match reference {
                    CompareRef::Nfs(_) => "nfs",
                    CompareRef::Baseline(_) => "baseline",
                    _ => "effective",
                };
                let Some(tenant) = group.tenants.first().cloned() else {
                    // A swimlane with no tenant has no file to read as: an absent side.
                    return Ok(Side::absent(kind));
                };
                let mut side = match reference {
                    CompareRef::Effective(_) => self.effective_side(group, file, &tenant, channel).await?,
                    CompareRef::Nfs(_) => match self.chosen_row(group, &tenant, file, channel).await? {
                        Some(row) => self.row_side(&row, "nfs", row.blob).await?,
                        None => Side::absent("nfs"),
                    },
                    _ => match self.chosen_row(group, &tenant, file, channel).await? {
                        Some(row) => {
                            let blob = self.baseline_blob(lane, &row.path).await?;
                            match blob {
                                Some(b) => self.row_side(&row, "baseline", Some(b)).await?,
                                None => Side::absent("baseline"),
                            }
                        }
                        None => Side::absent("baseline"),
                    },
                };
                side.tenant = Some((lane.clone(), tenant));
                Ok(side)
            }
            CompareRef::GitBase(git_ref) => self.git_side(RepoKind::Base, git_ref, file).await,
            CompareRef::GitTenant(git_ref) => self.git_side(RepoKind::Tenant, git_ref, file).await,
        }
    }

    async fn baseline_blob(
        &self,
        lane: &SwimlaneId,
        path: &NfsPath,
    ) -> Result<Option<ContentHash>, ReadError> {
        let blob: Option<Option<Vec<u8>>> = self
            .db
            .run(
                "read_baseline_blob",
                sqlx::query(BASELINE_BLOB_SQL)
                    .bind(lane.as_str())
                    .bind(path.as_str())
                    .map(|r: sqlx::postgres::PgRow| r.get::<Option<Vec<u8>>, _>(0))
                    .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(rows::opt_hash(blob.flatten().as_deref())?)
    }

    /// A side from a file row and the blob that holds its content (the row's own for `nfs`, the baseline's for `baseline`).
    async fn row_side(
        &self,
        row: &LayerRow,
        kind: &'static str,
        blob: Option<ContentHash>,
    ) -> Result<Side, ReadError> {
        let blob = blob.ok_or(ReadError::NotFound)?;
        if row.denied || row.class == FileClass::Denied {
            return Ok(Side {
                kind,
                class: FileClass::Denied,
                withheld: true,
                hash: Some(blob),
                size: Some(row.size),
                text: Text::None,
                rows: None,
                effective: None,
                syntax: None,
                tenant: None,
            });
        }
        let bytes = self.get_blob(&blob).await?.ok_or(ReadError::NotFound)?;
        let syntax = (row.class == FileClass::Structured)
            .then(|| Syntax::of_path(&row.path))
            .flatten();
        let rows = match syntax {
            Some(syntax) => self.rows_of(&[(blob, syntax)]).await?.remove(&blob),
            None => None,
        };
        Ok(Side {
            kind,
            class: row.class,
            withheld: false,
            hash: Some(blob),
            size: Some(bytes.len() as u64),
            text: Text::Bytes(bytes),
            rows,
            effective: None,
            syntax,
            tenant: None,
        })
    }

    async fn effective_side(
        &self,
        group: &Group,
        file: &LogicalFile,
        tenant: &TenantId,
        channel: Option<&ChannelName>,
    ) -> Result<Side, ReadError> {
        let l = layering(group, file, tenant);
        if l.role == FileRole::Resource {
            // A resource is the file the config-server chooses, as text.
            return match self.chosen_row(group, tenant, file, channel).await? {
                Some(row) => self.row_side(&row, "effective", row.blob).await,
                None => Ok(Side::absent("effective")),
            };
        }
        let computed = self.effectives_of(&[(group, file, tenant)]).await?;
        let Some((l, entry)) = computed.into_iter().next().flatten() else {
            return Ok(Side::absent("effective"));
        };
        if l.tenant.iter().chain(l.base.iter()).any(|r| r.denied) {
            return Ok(Side {
                withheld: true,
                class: FileClass::Denied,
                hash: Some(entry.effective.hash),
                ..Side::absent("effective")
            });
        }
        // The text of a property source is the file the merge is made of when there is only one, and no text when they merge.
        let single = entry.effective.sources.len() == 1;
        let winner = single
            .then(|| {
                l.tenant
                    .iter()
                    .chain(l.base.iter())
                    .find(|r| r.path == entry.effective.sources[0].path)
            })
            .flatten()
            .cloned();
        let text = match winner {
            Some(row) => match row.blob {
                Some(blob) => self.get_blob(&blob).await?.map_or(Text::None, Text::Bytes),
                None => Text::None,
            },
            None => Text::None,
        };
        let syntax = l
            .tenant
            .iter()
            .chain(l.base.iter())
            .next()
            .and_then(|r| Syntax::of_path(&r.path));
        Ok(Side {
            kind: "effective",
            class: FileClass::Structured,
            withheld: false,
            hash: Some(entry.effective.hash),
            size: None,
            text,
            rows: None,
            effective: Some(entry),
            syntax,
            tenant: None,
        })
    }

    async fn git_side(
        &self,
        repo: RepoKind,
        git_ref: &GitRef,
        file: &LogicalFile,
    ) -> Result<Side, ReadError> {
        let rule = self.mapping_rule().await?;
        let root = match repo {
            RepoKind::Base => rule.base_root(),
            RepoKind::Tenant => rule.tenant_root(),
        };
        let repo_path = RepoPath::parse(&format!("{}/{}", root.as_str(), file.as_str()))
            .map_err(|_| ReadError::BadRequest)?;
        // A mistyped ref is `NotFound` (nothing is compared with it); a file that is not in the tree at a ref that exists is an absent side.
        let Some(bytes) = self.git_read_opt(repo, git_ref, &repo_path).await? else {
            return Ok(Side::absent("git"));
        };
        let class = classify(file.as_path(), Some(&bytes), &self.deny);
        let hash = ports::content_hash(&bytes);
        if class == FileClass::Denied {
            return Ok(Side {
                kind: "git",
                class,
                withheld: true,
                hash: Some(hash),
                size: Some(bytes.len() as u64),
                text: Text::None,
                rows: None,
                effective: None,
                syntax: None,
                tenant: None,
            });
        }
        let syntax = (class == FileClass::Structured)
            .then(|| Syntax::of_path(file.as_path()))
            .flatten();
        let rows = match syntax {
            Some(s) => Some(self.rows_from_bytes(hash, s, bytes.clone()).await?),
            None => None,
        };
        Ok(Side {
            kind: "git",
            class,
            withheld: false,
            hash: Some(hash),
            size: Some(bytes.len() as u64),
            text: Text::Bytes(bytes),
            rows,
            effective: None,
            syntax,
            tenant: None,
        })
    }

    /// `compareFile`.
    pub async fn compare_file(
        &self,
        l: &CompareRef,
        r: &CompareRef,
        file: &LogicalFile,
        channel: Option<&ChannelName>,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<Named<Comparison>>, ReadError> {
        let (left, right) =
            tokio::try_join!(self.load_side(l, file, channel), self.load_side(r, file, channel))?;
        if left.is_absent() && right.is_absent() {
            return Err(ReadError::NotFound);
        }
        let tag = etag::tag(
            "compare",
            &[
                &l.to_string(),
                &r.to_string(),
                file.as_str(),
                &left.identity(),
                &right.identity(),
                left.kind,
                right.kind,
            ],
        );
        if etag::matches(if_none_match, &tag) {
            return Ok(Tagged::NotModified { etag: tag });
        }
        let key = {
            let mut h = Sha256::new();
            h.update(b"lanekeeper/diff-key/v1\n");
            for side in [&left, &right] {
                h.update(side.kind.as_bytes());
                h.update(side.hash.map_or([0_u8; 32], |h| *h.as_bytes()));
                h.update([u8::from(side.withheld)]);
            }
            ContentHash::from_bytes(h.finalize().into())
        };
        // Which tenant stood for each swimlane the references name (A30), known from the sides whatever the cache says.
        let tenants: Vec<(SwimlaneId, TenantId)> =
            [&left, &right].iter().filter_map(|s| s.tenant.clone()).collect();
        let cached = self.caches.diffs.get(&key);
        self.caches.count(CacheName::Diff, cached.is_some());
        let entry = if let Some(entry) = cached {
            entry
        } else {
            let limits = self.limits.clone();
            let (left, right) = (Arc::new(left), Arc::new(right));
            let body = tokio::task::spawn_blocking(move || diff(&left, &right, &limits))
                .await
                .map_err(|_| {
                    warn!("a diff task on the blocking pool did not finish");
                    ReadError::Unavailable
                })?;
            let entry = Arc::new(DiffEntry {
                weight: weight_of(&body),
                body,
            });
            self.caches.diffs.insert(key, Arc::clone(&entry));
            entry
        };
        Ok(finish(l, r, file, &tag, &entry, tenants))
    }
}

/// The answer from a cached diff: the references and the file go back in, and each swimlane's standing tenant is named once.
fn finish(
    l: &CompareRef,
    r: &CompareRef,
    file: &LogicalFile,
    tag: &str,
    entry: &DiffEntry,
    tenants: Vec<(SwimlaneId, TenantId)>,
) -> Tagged<Named<Comparison>> {
    let body = entry.body.clone();
    let comparison = Comparison {
        left: l.clone(),
        right: r.clone(),
        file: Some(file.clone()),
        class: body.class,
        identical: body.identical,
        eol_only: body.eol_only,
        hunks: body.hunks,
        setting_changes: body.setting_changes,
        binary: body.binary,
    };
    let mut seen: Vec<(SwimlaneId, TenantId)> = Vec::new();
    for t in tenants {
        if !seen.contains(&t) {
            seen.push(t);
        }
    }
    Tagged::Fresh {
        etag: tag.to_owned(),
        value: Named {
            value: comparison,
            tenants: seen,
        },
    }
}
