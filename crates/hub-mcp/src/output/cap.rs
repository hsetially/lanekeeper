//! The byte cap (P9, D58): a result is at most 16 KB for a read tool and 4 KB for a write tool, counted over everything the client
//! receives for it, the `structuredContent` and the `content` blocks.
//!
//! Sizes are measured, never estimated: [`json_len`] serialises into a counter and keeps no bytes. A list is cut by dropping its
//! trailing items, with the size of each item measured once ([`fit`]), and the finished result is measured once more by the caller
//! as the last check.

use std::io;

use serde::Serialize;

/// A writer that only counts.
#[derive(Debug, Default)]
struct Counter(usize);

impl io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The length of `value` as compact JSON, in bytes. A value that cannot be serialised counts as `usize::MAX`, so it never fits.
#[must_use]
pub fn json_len<T: Serialize + ?Sized>(value: &T) -> usize {
    let mut counter = Counter::default();
    if serde_json::to_writer(&mut counter, value).is_ok() {
        counter.0
    } else {
        usize::MAX
    }
}

/// The length of `text` as a JSON string, quotes included.
#[must_use]
pub fn json_string_len(text: &str) -> usize {
    2 + text
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            c if c < ' ' => 6,
            c => c.len_utf8(),
        })
        .sum::<usize>()
}

/// How many of the leading items fit in `budget` bytes of a JSON array: the sizes of the first `n` items plus one comma between
/// each pair. Zero when not even the first fits.
#[must_use]
pub fn fit(sizes: &[usize], budget: usize) -> usize {
    let mut used = 0_usize;
    for (i, size) in sizes.iter().enumerate() {
        let next = used.saturating_add(*size).saturating_add(usize::from(i > 0));
        if next > budget {
            return i;
        }
        used = next;
    }
    sizes.len()
}

/// The sizes of the items of a list, each measured once.
#[must_use]
pub fn sizes_of<T: Serialize>(items: &[T]) -> Vec<usize> {
    items.iter().map(json_len).collect()
}

/// The longest prefix of `text` of at most `max` characters, and whether anything was cut.
#[must_use]
pub fn cut_chars(text: &str, max: usize) -> (&str, bool) {
    match text.char_indices().nth(max) {
        Some((at, _)) => (&text[..at], true),
        None => (text, false),
    }
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary, and whether anything was cut.
#[must_use]
pub fn cut_bytes(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// The leading items of a list that fit in `budget` bytes as a JSON array, and whether any were dropped. For the lists of a result
/// that are not what the result pages (the sources of a file, its unresolved placeholders): each gets a share of the cap, so that a
/// list with long entries cannot take all of it.
#[must_use]
pub fn take_fitting<T: Serialize>(mut items: Vec<T>, budget: usize) -> (Vec<T>, bool) {
    let kept = fit(&sizes_of(&items), budget);
    let cut = kept < items.len();
    items.truncate(kept);
    (items, cut)
}
