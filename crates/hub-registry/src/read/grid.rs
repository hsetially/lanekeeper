//! The comparison grid (T6, P7: 150 ms, D65, A30, A31).
//!
//! One logical file across up to 64 swimlanes, rows are setting paths and columns are swimlanes. The effective config of the file in
//! each swimlane is read once (one statement for the layers of all of them, one for the setting rows of the blobs that are not
//! cached, one parallel pass of the read pool over the merges that are not cached), and `engine::grid` merges them. Swimlanes that
//! read the same files share one cached effective config, so the cost of the grid is the number of *distinct* configs and not of
//! swimlanes.
//!
//! # Which tenant a column is
//!
//! A swimlane has a set of tenants (Q34); the first in sorted order stands for it (A30), and the response names it (the REST edge
//! puts the pairs in the `X-Lanekeeper-Tenants` header, because the contract's `Grid` has no field for it). A swimlane that has
//! no tenant, or no copy of the file, is a column of "absent" cells.
//!
//! # Paging (keyset, A31)
//!
//! The rows are in setting-path order and a page resumes after the last path it showed, which a cursor holds when it is at most
//! [`MAX_PATH_KEY`] bytes. A longer path (up to 1,024) does not fit a cursor, so such a page ends with an *ordinal* cursor instead: the
//! number of rows already shown and a tag of the columns it was counted on; the grid is recomputed and the first rows skipped, and a
//! cursor whose tag no longer matches (the data changed under it) is refused. The ordinal is only ever used for those rare paths.

use std::collections::HashMap;
use std::sync::Arc;

use domain::{
    ContentHash, Grid, GridCell, GridQuery, GridRow, Paged, SettingLocation, SettingPath, SwimlaneId,
    TenantId,
};
use engine::{GridColumn, GridOptions, grid};
use ports::ReadError;
use sha2::{Digest, Sha256};
use tracing::warn;

use super::api::Named;
use super::cache::EffectiveEntry;
use super::cursor::{self, Context, Field, Kind};
use super::service::Inner;

/// The longest setting path a cursor carries as itself.
pub const MAX_PATH_KEY: usize = 300;

/// Where a page resumes.
enum Resume {
    Start,
    AfterPath(SettingPath),
    Skip { rows: usize, tag: Vec<u8> },
}

fn tag_of(effective_hashes: &[Option<ContentHash>]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"lanekeeper/grid-tag/v1\n");
    for hash in effective_hashes {
        h.update(hash.map_or([0_u8; 32], |h| *h.as_bytes()));
    }
    h.finalize()[..8].to_vec()
}

/// The effective config of each column, in the order asked for, and the tenant that stands for each swimlane.
type Columns = (Vec<Option<Arc<EffectiveEntry>>>, Vec<(SwimlaneId, TenantId)>);

impl Inner {
    /// The columns of a grid: validated, loaded (one statement for the layers of all swimlanes, one for the rows not cached, one parallel
    /// pass for the merges not cached).
    async fn grid_columns(&self, q: &GridQuery) -> Result<Columns, ReadError> {
        if q.swimlanes.is_empty() || q.swimlanes.len() > GridQuery::MAX_SWIMLANES {
            return Err(ReadError::BadRequest);
        }
        let mut unique: Vec<SwimlaneId> = Vec::new();
        for s in &q.swimlanes {
            if !unique.contains(s) {
                unique.push(s.clone());
            }
        }
        let groups = self.groups_of(&unique, &q.file).await?;
        if unique.iter().any(|s| !groups.contains_key(s)) {
            return Err(ReadError::NotFound);
        }

        // One standing tenant for each swimlane; a swimlane without one is a column of absent cells.
        let mut standing: Vec<(SwimlaneId, TenantId)> = Vec::new();
        let mut jobs = Vec::new();
        for s in &unique {
            let group = groups.get(s).ok_or(ReadError::NotFound)?;
            if let Some(t) = group.tenants.first() {
                standing.push((s.clone(), t.clone()));
                jobs.push((group, &q.file, t));
            }
        }
        let computed = self.effectives_of(&jobs).await?;
        let mut by_lane: HashMap<&SwimlaneId, Arc<EffectiveEntry>> = HashMap::new();
        for ((s, _), entry) in standing.iter().zip(computed) {
            if let Some((_, entry)) = entry {
                by_lane.insert(s, entry);
            }
        }
        let entries = q.swimlanes.iter().map(|s| by_lane.get(s).cloned()).collect();
        Ok((entries, standing))
    }

    pub async fn grid_view(&self, q: &GridQuery) -> Result<Named<Grid>, ReadError> {
        let (entries, standing) = self.grid_columns(q).await?;
        let hashes: Vec<Option<ContentHash>> = entries
            .iter()
            .map(|e| e.as_ref().map(|e| e.effective.hash))
            .collect();

        let key_parts: Vec<String> = q.swimlanes.iter().map(ToString::to_string).collect();
        let joined = key_parts.join(",");
        let context = Context::of(
            Kind::Grid,
            &[
                q.file.as_str(),
                &joined,
                q.channel.as_ref().map_or("", domain::ChannelName::as_str),
                if q.only_differing { "differing" } else { "all" },
            ],
        );
        let resume = match cursor::resume(&q.page, Kind::Grid, &context)? {
            None => Resume::Start,
            Some(fields) => match fields.as_slice() {
                [Field::Bytes(path)] => {
                    let text = String::from_utf8(path.clone()).map_err(|_| ReadError::BadRequest)?;
                    Resume::AfterPath(SettingPath::parse(&text).map_err(|_| ReadError::BadRequest)?)
                }
                [Field::Int(rows), Field::Bytes(tag)] => Resume::Skip {
                    rows: usize::try_from(*rows).map_err(|_| ReadError::BadRequest)?,
                    tag: tag.clone(),
                },
                _ => return Err(ReadError::BadRequest),
            },
        };
        let tag = tag_of(&hashes);
        if let Resume::Skip { tag: given, .. } = &resume {
            if *given != tag {
                // The columns changed since the page that issued this cursor: its position means nothing now.
                return Err(ReadError::BadRequest);
            }
        }

        let (swimlanes, channel, only_differing, limit) = (
            q.swimlanes.clone(),
            q.channel.clone(),
            q.only_differing,
            q.page.limit() as usize,
        );
        let limits = self.limits.clone();
        let page = tokio::task::spawn_blocking(move || {
            run_grid(
                &swimlanes,
                &entries,
                channel.as_ref(),
                only_differing,
                limit,
                &resume,
                &limits,
                &tag,
            )
        })
        .await
        .map_err(|_| {
            warn!("a grid task on the blocking pool did not finish");
            ReadError::Unavailable
        })??;

        let next = match page.next {
            Some(key) => Some(cursor::issue(Kind::Grid, &context, key)?),
            None => None,
        };
        Ok(Named {
            value: Grid {
                swimlanes: q.swimlanes.clone(),
                rows: Paged {
                    items: page.rows,
                    next,
                },
            },
            tenants: standing,
        })
    }
}

struct GridPage {
    rows: Vec<GridRow>,
    next: Option<Vec<Field>>,
}

#[allow(clippy::too_many_arguments)]
fn run_grid(
    swimlanes: &[SwimlaneId],
    entries: &[Option<Arc<EffectiveEntry>>],
    channel: Option<&domain::ChannelName>,
    only_differing: bool,
    limit: usize,
    resume: &Resume,
    limits: &engine::Limits,
    tag: &[u8],
) -> Result<GridPage, ReadError> {
    let columns: Vec<GridColumn<'_>> = swimlanes
        .iter()
        .zip(entries)
        .map(|(swimlane, e)| GridColumn {
            swimlane,
            effective: e.as_ref().map(|e| &e.effective),
        })
        .collect();
    let (after, skip, ask) = match resume {
        Resume::Start => (None, 0, limit),
        Resume::AfterPath(p) => (Some(p), 0, limit),
        Resume::Skip { rows, .. } => (None, *rows, rows.saturating_add(limit)),
    };
    let result = grid(
        &columns,
        &GridOptions {
            channel,
            only_differing,
            after,
            // One more than a page: whether there is a next page is whether it came back.
            max_rows: ask.saturating_add(1),
        },
        limits,
    )
    .map_err(|e| {
        warn!(?e, "the engine refused a grid");
        ReadError::BadRequest
    })?;
    let all = &result.rows[skip.min(result.rows.len())..];
    let has_more = all.len() > limit;
    let shown = &all[..all.len().min(limit)];
    let rows: Vec<GridRow> = shown
        .iter()
        .map(|row| GridRow {
            path: row.path.clone(),
            cells: row
                .cells
                .iter()
                .map(|cell| match cell {
                    Some(v) => GridCell {
                        value: Some(v.value.clone()),
                        location: Some(SettingLocation {
                            path: row.path.clone(),
                            lines: v.lines,
                        }),
                    },
                    None => GridCell {
                        value: None,
                        location: None,
                    },
                })
                .collect(),
        })
        .collect();
    let next = if has_more {
        match shown.last() {
            Some(last) if last.path.as_str().len() <= MAX_PATH_KEY => {
                Some(vec![Field::text(last.path.as_str())])
            }
            Some(last) => {
                // A path too long for a cursor: the position is the number of rows before the next one, counted on the columns as
                // they are now (the tag says so). A page that resumed by path has to count the rows before it.
                let before = match resume {
                    Resume::Start => 0,
                    Resume::Skip { rows, .. } => *rows,
                    Resume::AfterPath(after) => {
                        rows_through(&columns, channel, only_differing, after, limits)?
                    }
                };
                Some(vec![
                    Field::Int(i64::try_from(before + shown.len()).unwrap_or(i64::MAX)),
                    Field::Bytes(tag.to_vec()),
                ])
                .filter(|_| !last.path.as_str().is_empty())
            }
            None => None,
        }
    } else {
        None
    };
    Ok(GridPage { rows, next })
}

/// How many rows of the grid are at or before `path`: the position of a page that resumed by path, for an ordinal cursor.
fn rows_through(
    columns: &[GridColumn<'_>],
    channel: Option<&domain::ChannelName>,
    only_differing: bool,
    path: &SettingPath,
    limits: &engine::Limits,
) -> Result<usize, ReadError> {
    let all = grid(
        columns,
        &GridOptions {
            channel,
            only_differing,
            after: None,
            max_rows: usize::MAX,
        },
        limits,
    )
    .map_err(|_| ReadError::BadRequest)?;
    Ok(all.rows.partition_point(|r| r.path <= path))
}
