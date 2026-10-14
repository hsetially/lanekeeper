//! Keyset cursors (A31, S11, rule 5).
//!
//! A cursor is the position after the last row of a page, and nothing else: a few small numbers and short byte strings, never a
//! path (a path is up to 1,024 bytes and a setting path up to 2,048, but a `domain::Cursor` is at most 512 characters). Each list
//! says what its key is: the child name of a tree (at most 255 bytes), `file_state.seq` for the drift list, the time and id of a
//! version, the id of a finding.
//!
//! # What a cursor can and cannot do
//!
//! The request, never the cursor, names the data: the swimlane comes from the path of the URL, the filters from the query string,
//! and the caller's access is checked on both before a cursor is looked at. A cursor only says where in *that* list to resume, so a
//! forged one can choose a position in a list the caller may read anyway and nothing else (roles are global, D35, so there is no
//! per-swimlane ACL for it to get around). On top of that every cursor is bound to the query it was issued for: it carries a
//! context tag, the first eight bytes of a SHA-256 over the kind of list and the parameters that select its rows. A cursor from
//! another swimlane, another filter or another list is refused (400) instead of resuming somewhere that makes no sense. The tag is
//! not a MAC (the registry holds no secret for it), so it stops mistakes and replays across queries, not a caller who computes the
//! tag for a query of their own: they gain nothing by it, which is the point above.
//!
//! # Wire format
//!
//! `base64url(no padding)` of `version(1) kind(1) context(8) count(1) field*`, a field being `0 i64_be(8)` or `1 u16_be(len) bytes`.
//! The decoder is strict: an unknown version, kind or tag, a short or long body, trailing bytes, or a base64 text that is not the
//! canonical encoding of its bytes is refused, so a cursor has exactly one text and re-encoding what was accepted gives it back.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::{Cursor, Page};
use ports::ReadError;
use sha2::{Digest, Sha256};

/// Bump when the layout changes: a cursor of another version is refused.
const VERSION: u8 = 1;
/// The most fields in a key. No list needs more than four.
const MAX_FIELDS: usize = 6;
/// The longest byte string in a field, in bytes.
pub(crate) const MAX_BYTES_FIELD: usize = 255;
/// The raw bytes a 512-character cursor can hold.
const MAX_RAW: usize = 384;

/// What a cursor pages over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Swimlanes = 1,
    Tree = 2,
    Drift = 3,
    History = 4,
    Findings = 5,
    SettingsSearch = 6,
    Grid = 7,
    CompareTree = 8,
    Restarts = 9,
}

impl Kind {
    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Swimlanes,
            2 => Self::Tree,
            3 => Self::Drift,
            4 => Self::History,
            5 => Self::Findings,
            6 => Self::SettingsSearch,
            7 => Self::Grid,
            8 => Self::CompareTree,
            9 => Self::Restarts,
            _ => return None,
        })
    }
}

/// One component of a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Int(i64),
    Bytes(Vec<u8>),
}

impl Field {
    pub fn text(s: &str) -> Self {
        Self::Bytes(s.as_bytes().to_vec())
    }
}

/// Why a cursor was refused. Carries nothing of the input (S21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("the cursor is malformed")]
    Malformed,
    #[error("the cursor is for another list")]
    WrongKind,
    #[error("the cursor is for another query")]
    WrongContext,
    #[error("the key does not fit a cursor")]
    TooLarge,
}

/// The tag that binds a cursor to the parameters of its query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context([u8; 8]);

impl Context {
    /// `parts` are everything that selects the rows of the list (the swimlane, the prefix, the filters, in a fixed order). Each is
    /// length-prefixed, so a byte moved from one part to the next is another context.
    pub fn of(kind: Kind, parts: &[&str]) -> Self {
        let mut h = Sha256::new();
        h.update([kind as u8]);
        for part in parts {
            h.update((part.len() as u64).to_be_bytes());
            h.update(part.as_bytes());
        }
        let digest = h.finalize();
        let mut tag = [0_u8; 8];
        tag.copy_from_slice(&digest[..8]);
        Self(tag)
    }
}

/// A cursor taken apart without a context to check: the fuzz target's entry point, and the first step of [`decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub kind: Kind,
    pub context: [u8; 8],
    pub fields: Vec<Field>,
}

/// Take a cursor apart. Total: any text is `Ok` or `Err`, never a panic.
pub fn parse(cursor: &str) -> Result<Parsed, CursorError> {
    let raw = URL_SAFE_NO_PAD
        .decode(cursor.as_bytes())
        .map_err(|_| CursorError::Malformed)?;
    if raw.len() > MAX_RAW || raw.len() < 11 {
        return Err(CursorError::Malformed);
    }
    let [version, kind, rest @ ..] = raw.as_slice() else {
        return Err(CursorError::Malformed);
    };
    if *version != VERSION {
        return Err(CursorError::Malformed);
    }
    let kind = Kind::from_byte(*kind).ok_or(CursorError::Malformed)?;
    let (context, rest) = rest.split_at(8);
    let (count, mut body) = rest.split_first().ok_or(CursorError::Malformed)?;
    let count = usize::from(*count);
    if count > MAX_FIELDS {
        return Err(CursorError::Malformed);
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let (tag, after) = body.split_first().ok_or(CursorError::Malformed)?;
        match tag {
            0 => {
                let (n, after) = after.split_at_checked(8).ok_or(CursorError::Malformed)?;
                let n: [u8; 8] = n.try_into().map_err(|_| CursorError::Malformed)?;
                fields.push(Field::Int(i64::from_be_bytes(n)));
                body = after;
            }
            1 => {
                let (len, after) = after.split_at_checked(2).ok_or(CursorError::Malformed)?;
                let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
                if len > MAX_BYTES_FIELD {
                    return Err(CursorError::Malformed);
                }
                let (bytes, after) = after.split_at_checked(len).ok_or(CursorError::Malformed)?;
                fields.push(Field::Bytes(bytes.to_vec()));
                body = after;
            }
            _ => return Err(CursorError::Malformed),
        }
    }
    if !body.is_empty() {
        return Err(CursorError::Malformed);
    }
    let mut tag = [0_u8; 8];
    tag.copy_from_slice(context);
    let parsed = Parsed {
        kind,
        context: tag,
        fields,
    };
    // One text for one cursor: the canonical encoding of what was read is the text that was given.
    if assemble(&parsed)?.as_str() != cursor {
        return Err(CursorError::Malformed);
    }
    Ok(parsed)
}

/// Put a cursor together from its parts.
pub fn assemble(p: &Parsed) -> Result<Cursor, CursorError> {
    if p.fields.len() > MAX_FIELDS {
        return Err(CursorError::TooLarge);
    }
    let mut raw = Vec::with_capacity(64);
    raw.push(VERSION);
    raw.push(p.kind as u8);
    raw.extend_from_slice(&p.context);
    raw.push(u8::try_from(p.fields.len()).map_err(|_| CursorError::TooLarge)?);
    for field in &p.fields {
        match field {
            Field::Int(n) => {
                raw.push(0);
                raw.extend_from_slice(&n.to_be_bytes());
            }
            Field::Bytes(b) => {
                if b.len() > MAX_BYTES_FIELD {
                    return Err(CursorError::TooLarge);
                }
                raw.push(1);
                raw.extend_from_slice(
                    &u16::try_from(b.len())
                        .map_err(|_| CursorError::TooLarge)?
                        .to_be_bytes(),
                );
                raw.extend_from_slice(b);
            }
        }
    }
    if raw.len() > MAX_RAW {
        return Err(CursorError::TooLarge);
    }
    Cursor::parse(&URL_SAFE_NO_PAD.encode(&raw)).map_err(|_| CursorError::TooLarge)
}

/// The cursor of a position in the list `kind` selected by `context`.
pub fn encode(kind: Kind, context: &Context, fields: Vec<Field>) -> Result<Cursor, CursorError> {
    assemble(&Parsed {
        kind,
        context: context.0,
        fields,
    })
}

/// The position a cursor names, in the list `kind` selected by `context`.
pub fn decode(cursor: &Cursor, kind: Kind, context: &Context) -> Result<Vec<Field>, CursorError> {
    let parsed = parse(cursor.as_str())?;
    if parsed.kind != kind {
        return Err(CursorError::WrongKind);
    }
    if parsed.context != context.0 {
        return Err(CursorError::WrongContext);
    }
    Ok(parsed.fields)
}

/// The key the request's cursor names, in the list `kind` selected by `context`: `None` for a first page, and `BadRequest` for a cursor
/// that is malformed or that was issued for another list or query.
pub fn resume(page: &Page, kind: Kind, context: &Context) -> Result<Option<Vec<Field>>, ReadError> {
    page.cursor()
        .map(|c| decode(c, kind, context).map_err(|_| ReadError::BadRequest))
        .transpose()
}

/// The cursor of the position after the last row of a page. A key that does not fit is a bug of ours, never the caller's: it is
/// logged as a fixed word and the read is unavailable, so that a page never carries a cursor that cannot be followed.
pub fn issue(kind: Kind, context: &Context, fields: Vec<Field>) -> Result<Cursor, ReadError> {
    encode(kind, context, fields).map_err(|e| {
        tracing::warn!(?e, "a cursor key does not fit");
        ReadError::Unavailable
    })
}

/// The fields as the shapes a list expects, or `Malformed` when the count or a type is not what that list writes.
pub fn int(fields: &[Field], at: usize) -> Result<i64, CursorError> {
    match fields.get(at) {
        Some(Field::Int(n)) => Ok(*n),
        _ => Err(CursorError::Malformed),
    }
}

pub fn text(fields: &[Field], at: usize) -> Result<String, CursorError> {
    match fields.get(at) {
        Some(Field::Bytes(b)) => String::from_utf8(b.clone()).map_err(|_| CursorError::Malformed),
        _ => Err(CursorError::Malformed),
    }
}

pub fn bytes(fields: &[Field], at: usize) -> Result<&[u8], CursorError> {
    match fields.get(at) {
        Some(Field::Bytes(b)) => Ok(b),
        _ => Err(CursorError::Malformed),
    }
}

/// A key that has exactly `n` fields.
pub fn arity(fields: &[Field], n: usize) -> Result<(), CursorError> {
    if fields.len() == n {
        Ok(())
    } else {
        Err(CursorError::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ctx(what: &str) -> Context {
        Context::of(Kind::Tree, &["sit1", what])
    }

    #[test]
    fn a_cursor_round_trips_and_has_one_text() {
        let fields = vec![
            Field::text("tx-infinity-api"),
            Field::Int(-42),
            Field::Int(i64::MAX),
        ];
        let c = encode(Kind::Tree, &ctx("a"), fields.clone()).unwrap();
        assert!(c.as_str().len() <= 512);
        assert_eq!(decode(&c, Kind::Tree, &ctx("a")).unwrap(), fields);
        let p = parse(c.as_str()).unwrap();
        assert_eq!(assemble(&p).unwrap(), c, "re-encoding gives the same text");
    }

    #[test]
    fn a_cursor_of_another_list_or_query_is_refused() {
        let c = encode(Kind::Tree, &ctx("a"), vec![Field::Int(1)]).unwrap();
        assert_eq!(decode(&c, Kind::Drift, &ctx("a")), Err(CursorError::WrongKind));
        assert_eq!(decode(&c, Kind::Tree, &ctx("b")), Err(CursorError::WrongContext));
        let other_lane = Context::of(Kind::Tree, &["sit2", "a"]);
        assert_eq!(
            decode(&c, Kind::Tree, &other_lane),
            Err(CursorError::WrongContext),
            "a cursor of one swimlane does not page another"
        );
    }

    #[test]
    fn a_byte_moved_between_context_parts_is_another_context() {
        assert_ne!(
            Context::of(Kind::Tree, &["ab", "c"]),
            Context::of(Kind::Tree, &["a", "bc"])
        );
    }

    #[test]
    fn the_longest_key_fits_in_512_characters() {
        // A31: the widest key any list writes is a child name of 255 bytes with a time and an id beside it.
        let c = encode(
            Kind::Tree,
            &ctx("a"),
            vec![
                Field::Bytes(vec![b'x'; MAX_BYTES_FIELD]),
                Field::Int(i64::MIN),
                Field::Int(i64::MAX),
            ],
        )
        .unwrap();
        assert!(c.as_str().len() <= 512, "{} characters", c.as_str().len());
        let too_wide = encode(
            Kind::Tree,
            &ctx("a"),
            vec![Field::Bytes(vec![b'x'; MAX_BYTES_FIELD + 1])],
        );
        assert_eq!(too_wide, Err(CursorError::TooLarge));
        // Two of the widest strings do not fit 512 characters: refused, never truncated.
        let two = encode(
            Kind::Tree,
            &ctx("a"),
            vec![Field::Bytes(vec![b'x'; MAX_BYTES_FIELD]); 2],
        );
        assert_eq!(two, Err(CursorError::TooLarge));
    }

    #[test]
    fn tampering_is_refused_never_followed() {
        let c = encode(Kind::Drift, &ctx("a"), vec![Field::Int(7)]).unwrap();
        let text = c.as_str();
        // Every single-character change either decodes to the same list and query with another key (a position, harmless) or is
        // refused; none may panic.
        for i in 0..text.len() {
            let mut bytes = text.as_bytes().to_vec();
            bytes[i] = if bytes[i] == b'A' { b'B' } else { b'A' };
            let changed = String::from_utf8(bytes).unwrap();
            let _ = parse(&changed);
        }
        // Trailing bytes, a missing byte, an unknown version and an unknown kind.
        let raw = URL_SAFE_NO_PAD.decode(text).unwrap();
        for bad in [
            [raw.clone(), vec![0]].concat(),
            raw[..raw.len() - 1].to_vec(),
            [vec![2], raw[1..].to_vec()].concat(),
            [vec![1, 99], raw[2..].to_vec()].concat(),
            Vec::new(),
        ] {
            assert!(parse(&URL_SAFE_NO_PAD.encode(&bad)).is_err());
        }
        assert!(parse("").is_err());
        assert!(parse("!!!").is_err());
        assert!(parse(&"A".repeat(600)).is_err());
    }

    #[test]
    fn a_text_that_is_not_the_canonical_encoding_is_refused() {
        let c = encode(Kind::Drift, &ctx("a"), vec![Field::Int(7)]).unwrap();
        // Standard base64 padding on the same bytes is another text for the same cursor: refused.
        let padded = format!("{}=", c.as_str());
        assert!(parse(&padded).is_err());
    }

    proptest! {
        #[test]
        fn encode_then_decode_is_the_identity(
            ints in proptest::collection::vec(any::<i64>(), 0..3),
            texts in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..100), 0..2),
            lane in "[a-z0-9-]{1,20}",
        ) {
            let mut fields: Vec<Field> = ints.into_iter().map(Field::Int).collect();
            fields.extend(texts.into_iter().map(Field::Bytes));
            let context = Context::of(Kind::History, &[&lane]);
            let c = encode(Kind::History, &context, fields.clone()).unwrap();
            prop_assert_eq!(decode(&c, Kind::History, &context).unwrap(), fields);
        }

        #[test]
        fn decode_never_panics_and_what_it_accepts_re_encodes_to_itself(text in "[A-Za-z0-9_=.-]{0,600}") {
            if let Ok(p) = parse(&text) {
                let again = assemble(&p).unwrap();
                prop_assert_eq!(again.as_str(), text.as_str());
            }
        }

        #[test]
        fn arbitrary_bytes_as_a_cursor_are_total(raw in proptest::collection::vec(any::<u8>(), 0..400)) {
            let text = URL_SAFE_NO_PAD.encode(&raw);
            if let Ok(p) = parse(&text) {
                let again = assemble(&p).unwrap();
                prop_assert_eq!(again.as_str(), text.as_str());
            }
        }
    }
}
