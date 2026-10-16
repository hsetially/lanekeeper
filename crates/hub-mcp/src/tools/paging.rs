//! Paging that never loses or repeats an item (D58, S11): what a tool does with a `cursor` and a `limit`, and how it cuts a page that
//! the cap made too long.
//!
//! The ports page by their own keyset cursors. A page that the byte cap cuts short cannot be continued with the port's cursor (it
//! points past the whole page), so the cursor this server hands out remembers where the *port's* page began and how many of its
//! items were served ([`crate::output::cursor`]). The next call asks the port for that page again, with room for the items already
//! served, and leaves them out. Whatever the page size and the cap, every item is served once, in order
//! (`output_cap::paging_is_exact_prop`).
//!
//! A port that returns its whole, bounded result (`swimlanes`, `pending_restarts`) is paged by key in memory instead: the cursor
//! holds the key of the last item served.

use domain::{Cursor, Page};
use serde::Serialize;
use serde_json::Value;

use super::error::{Code, ToolError};
use crate::output::cap::sizes_of;
use crate::output::cursor::{self, MAX_SKIP};
use crate::output::envelope::{Parts, Rendered, Shape, fit_list};

/// The page size when a call gives none.
pub const DEFAULT_LIMIT: u32 = 50;

/// Where a call starts.
#[derive(Debug, Clone)]
pub struct Start {
    /// The port's cursor of the page to ask for, or the last key served (for a key-paged tool).
    pub from: Option<String>,
    /// Items of that page already served.
    pub skip: u32,
    /// The page size the caller asked for.
    pub limit: u32,
}

impl Start {
    /// Read the arguments of a paging call for `tag` (the tool's name).
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a cursor that this server did not make for this tool.
    pub fn read(tag: &str, cursor: Option<&str>, limit: Option<u32>) -> Result<Self, ToolError> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let Some(cursor) = cursor else {
            return Ok(Self {
                from: None,
                skip: 0,
                limit,
            });
        };
        let wrapped = cursor::decode(cursor, tag).map_err(|_| {
            ToolError::with_detail(
                Code::InvalidArgument,
                "The cursor is not one this tool made. Pass the `next` of the previous result.",
            )
        })?;
        Ok(Self {
            from: wrapped.inner,
            skip: wrapped.skip,
            limit,
        })
    }

    /// The page to ask the port for: the page that began at `from`, with room for the items already served.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a cursor the port cannot read (the port cursor inside is checked by its parser).
    pub fn port_page(&self) -> Result<Page, ToolError> {
        let cursor = self
            .from
            .as_deref()
            .map(Cursor::parse)
            .transpose()
            .map_err(|_| ToolError::new(Code::InvalidArgument))?;
        Page::new(self.limit.saturating_add(self.skip).min(Page::MAX_LIMIT), cursor)
            .map_err(|_| ToolError::new(Code::InvalidArgument))
    }

    /// For a key-paged tool: the key to continue after.
    #[must_use]
    pub fn after_key(&self) -> Option<&str> {
        self.from.as_deref()
    }
}

/// How the page that follows the one served is reached.
#[derive(Debug, Clone)]
pub enum Resume {
    /// The port's page began at `Start::from`, `Start::skip` items of it were left out, and the port says `port_next` follows it.
    Port { port_next: Option<String> },
    /// The tool pages by key in memory: `keys` are the keys of the candidate items, in order, and `more` says that items follow
    /// the candidates.
    Keys { keys: Vec<String>, more: bool },
}

/// What a paged tool says about its page, for the summary.
#[derive(Debug, Clone, Copy)]
pub struct Shown {
    /// Items in the result.
    pub shown: usize,
    /// Items that were candidates for it.
    pub candidates: usize,
    /// More exist than are in the result.
    pub more: bool,
}

/// Build the result of a paged tool.
///
/// `data(shown)` makes the tool's `data` from the leading items that fit; `summary` is written from counts only; `link` is the web
/// page that shows the rest. Returns the finished envelope.
// `link` is handed over by value because every tool builds it for this call alone; the result clones it for each size it tries.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub fn paged_result<T: Serialize>(
    shape: &Shape,
    tag: &str,
    start: &Start,
    resume: &Resume,
    items: &[T],
    data: impl Fn(&[T]) -> Value,
    summary: impl Fn(Shown) -> String,
    link: Option<String>,
) -> Rendered {
    let sizes = sizes_of(items);
    let build = |n: usize| {
        let more_in_page = n < items.len();
        // The cursor for what follows the first `n` items.
        let (next, too_long): (Option<String>, bool) = if n == 0 && !items.is_empty() {
            // Not even the first item fits: there is no way forward from here, and the link is the way out.
            (None, true)
        } else {
            match resume {
                Resume::Port { port_next } => {
                    if more_in_page {
                        let skip = start.skip.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
                        let made = (skip <= MAX_SKIP)
                            .then(|| cursor::encode(tag, start.from.as_deref(), skip))
                            .flatten();
                        let failed = made.is_none();
                        (made, failed)
                    } else if let Some(port) = port_next {
                        let made = cursor::encode(tag, Some(port), 0);
                        let failed = made.is_none();
                        (made, failed)
                    } else {
                        (None, false)
                    }
                }
                Resume::Keys { keys, more } => {
                    if more_in_page || *more {
                        let made = keys
                            .get(n.saturating_sub(1))
                            .and_then(|key| cursor::encode(tag, Some(key), 0));
                        let failed = made.is_none();
                        (made, failed)
                    } else {
                        (None, false)
                    }
                }
            }
        };
        let more_overall = more_in_page
            || match resume {
                Resume::Port { port_next } => port_next.is_some(),
                Resume::Keys { more, .. } => *more,
            };
        let truncated = more_overall || too_long;
        Parts {
            summary: summary(Shown {
                shown: n,
                candidates: items.len(),
                more: more_overall,
            }),
            data: data(&items[..n]),
            truncated,
            next,
            web_link: if truncated { link.clone() } else { None },
        }
    };
    fit_list(shape, &sizes, build).1
}

/// Build the result of a tool that does not page but whose list can be cut by the cap (`truncated` and `web_link`, no cursor).
///
/// `more` says that the tool itself left items out before the cap (a limit of its own, such as 100 services).
// `link` by value for the same reason as in `paged_result`.
#[allow(clippy::needless_pass_by_value)]
pub fn capped_result<T: Serialize>(
    shape: &Shape,
    items: &[T],
    more: bool,
    data: impl Fn(&[T]) -> Value,
    summary: impl Fn(Shown) -> String,
    link: Option<String>,
) -> Rendered {
    let sizes = sizes_of(items);
    let build = |n: usize| {
        let truncated = n < items.len() || more;
        Parts {
            summary: summary(Shown {
                shown: n,
                candidates: items.len(),
                more: truncated,
            }),
            data: data(&items[..n]),
            truncated,
            next: None,
            web_link: if truncated { link.clone() } else { None },
        }
    };
    fit_list(shape, &sizes, build).1
}
