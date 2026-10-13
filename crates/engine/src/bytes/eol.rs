//! Line endings (D17, rule 11).
//!
//! Real files are mostly CRLF, and a comparison must not see a difference where only the line endings differ.
//! [`normalise_eol`] turns every line terminator into LF; [`apply_eol`] writes them back in a chosen style. A file is
//! written back in the style it had ([`dominant_eol`] finds it), so an edit never rewrites the line endings of the lines
//! it did not touch.
//!
//! A terminator is CRLF, LF, or a lone CR (CR that is not followed by LF). A lone CR is a line break to YAML and to most
//! editors, so it is treated as one.
//!
//! # Encodings
//!
//! A UTF-16 file (recognised by its byte order mark alone) is read in two-byte units, so that the bytes `0D 0A` inside
//! a character are never taken for a line break. Every other file is read byte by byte, which is right for UTF-8 and
//! Latin-1: CR and LF are single bytes there and cannot occur inside another character. The view is decided by the
//! first two bytes, and rewriting line endings never changes them, so a file keeps the same view before and after.
//!
//! # Mixed files
//!
//! A file with more than one kind of terminator is `Mixed`. It is not restored line by line: [`dominant_eol`] picks
//! the style that most of its lines had, and that is what [`apply_eol`] gives it back.

use bytes::Bytes;
use domain::EolStyle;

/// The line ending a file is written with. A file has one, even when the original mixed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    Lf,
    Crlf,
}

/// How the bytes of a file are cut into code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Units {
    Byte,
    Utf16Le,
    Utf16Be,
}

impl Units {
    /// The view of a file: UTF-16 when it starts with a UTF-16 byte order mark, bytes otherwise.
    fn of(data: &[u8]) -> Self {
        match data {
            [0xFF, 0xFE, ..] => Self::Utf16Le,
            [0xFE, 0xFF, ..] => Self::Utf16Be,
            _ => Self::Byte,
        }
    }

    /// Bytes before the first code unit: the byte order mark of a UTF-16 file.
    fn skip(self) -> usize {
        match self {
            Self::Byte => 0,
            Self::Utf16Le | Self::Utf16Be => 2,
        }
    }

    fn width(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Utf16Le | Self::Utf16Be => 2,
        }
    }

    fn lf(self) -> &'static [u8] {
        match self {
            Self::Byte => b"\n",
            Self::Utf16Le => b"\n\0",
            Self::Utf16Be => b"\0\n",
        }
    }

    fn crlf(self) -> &'static [u8] {
        match self {
            Self::Byte => b"\r\n",
            Self::Utf16Le => b"\r\0\n\0",
            Self::Utf16Be => b"\0\r\0\n",
        }
    }

    fn is_cr(self, unit: &[u8]) -> bool {
        match self {
            Self::Byte => unit == b"\r",
            Self::Utf16Le => unit == b"\r\0",
            Self::Utf16Be => unit == b"\0\r",
        }
    }

    fn is_lf(self, unit: &[u8]) -> bool {
        unit == self.lf()
    }
}

/// One kind of line terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    Crlf,
    Lf,
    Cr,
}

/// The line terminators of a file, in order, as (byte offset, kind). A trailing odd byte of a UTF-16 file is not a unit
/// and is never part of a terminator.
struct Terminators<'a> {
    data: &'a [u8],
    units: Units,
    pos: usize,
}

impl<'a> Terminators<'a> {
    fn new(data: &'a [u8], units: Units) -> Self {
        Self {
            data,
            units,
            pos: units.skip(),
        }
    }

    fn unit(&self, at: usize) -> Option<&'a [u8]> {
        self.data.get(at..at + self.units.width())
    }
}

impl Iterator for Terminators<'_> {
    type Item = (usize, Term);

    fn next(&mut self) -> Option<Self::Item> {
        let width = self.units.width();
        if width == 1 {
            // Skip ahead to the next CR or LF.
            let rest = self.data.get(self.pos..)?;
            self.pos += rest.iter().position(|&c| c == b'\r' || c == b'\n')?;
        }
        while let Some(unit) = self.unit(self.pos) {
            let at = self.pos;
            self.pos += width;
            if self.units.is_lf(unit) {
                return Some((at, Term::Lf));
            }
            if self.units.is_cr(unit) {
                if self.unit(self.pos).is_some_and(|next| self.units.is_lf(next)) {
                    self.pos += width;
                    return Some((at, Term::Crlf));
                }
                return Some((at, Term::Cr));
            }
        }
        None
    }
}

/// How many terminators of each kind a file has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    crlf: usize,
    lf: usize,
    cr: usize,
}

impl Counts {
    fn of(data: &[u8], units: Units) -> Self {
        let mut counts = Self::default();
        for (_, term) in Terminators::new(data, units) {
            match term {
                Term::Crlf => counts.crlf += 1,
                Term::Lf => counts.lf += 1,
                Term::Cr => counts.cr += 1,
            }
        }
        counts
    }
}

/// The style of a file's line endings, as the domain names it. A file with no line ending at all is `Lf`, and a file with
/// a lone CR is `Mixed` (there is no style for it, and it is not LF or CRLF).
pub(crate) fn eol_style(data: &[u8]) -> EolStyle {
    let counts = Counts::of(data, Units::of(data));
    if counts.cr == 0 && counts.crlf == 0 {
        EolStyle::Lf
    } else if counts.cr == 0 && counts.lf == 0 {
        EolStyle::Crlf
    } else {
        EolStyle::Mixed
    }
}

/// The line ending to give this file when it is written again: CRLF when CRLF terminators outnumber the rest together
/// (LF and lone CR), LF otherwise. A file with no line endings, or with as many of each, gets LF.
pub fn dominant_eol(data: &[u8]) -> Newline {
    let counts = Counts::of(data, Units::of(data));
    if counts.crlf > counts.lf + counts.cr {
        Newline::Crlf
    } else {
        Newline::Lf
    }
}

/// Every line terminator of `data` as LF, so that two files that differ only in their line endings are equal. The byte
/// order mark and everything else are kept. When there is nothing to change, the result is the same buffer as `data`.
pub fn normalise_eol(data: &Bytes) -> Bytes {
    rewrite(data, Newline::Lf)
}

/// Every line terminator of `data` in the given style. A line that already has it is not doubled, and a lone CR becomes a
/// full terminator. Applying a style twice is the same as applying it once. When there is nothing to change, the result
/// is the same buffer as `data`.
pub fn apply_eol(data: &Bytes, newline: Newline) -> Bytes {
    rewrite(data, newline)
}

fn rewrite(data: &Bytes, to: Newline) -> Bytes {
    let units = Units::of(data);
    // The common case, a file that is already LF, is one pass for a CR.
    if to == Newline::Lf && !has_cr(data, units) {
        return data.clone();
    }
    let counts = Counts::of(data, units);
    let conforms = match to {
        Newline::Lf => counts.crlf == 0 && counts.cr == 0,
        Newline::Crlf => counts.lf == 0 && counts.cr == 0,
    };
    if conforms {
        return data.clone();
    }
    let (target, growth) = match to {
        Newline::Lf => (units.lf(), 0),
        // Each LF and each lone CR gains one unit.
        Newline::Crlf => (units.crlf(), (counts.lf + counts.cr) * units.width()),
    };
    let mut out = Vec::with_capacity(data.len() + growth);
    out.extend_from_slice(data.get(..units.skip()).unwrap_or_default());
    let mut copied = units.skip();
    for (at, term) in Terminators::new(data, units) {
        out.extend_from_slice(data.get(copied..at).unwrap_or_default());
        out.extend_from_slice(target);
        copied = at + units.width() * if term == Term::Crlf { 2 } else { 1 };
    }
    out.extend_from_slice(data.get(copied..).unwrap_or_default());
    Bytes::from(out)
}

fn has_cr(data: &[u8], units: Units) -> bool {
    match units {
        Units::Byte => data.contains(&b'\r'),
        Units::Utf16Le | Units::Utf16Be => data
            .get(units.skip()..)
            .unwrap_or_default()
            .chunks_exact(units.width())
            .any(|unit| units.is_cr(unit)),
    }
}
