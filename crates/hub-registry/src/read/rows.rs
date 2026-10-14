//! Reading stored values back into domain types (T6). A value the registry wrote always parses; one that does not is
//! [`RegistryError::Corrupt`], logged by the caller as a fixed word, never shown (S21).

use domain::{
    Attribution, AttributionSource, Confidence, ContentHash, DriftState, EolStyle, FileClass, FileFormat,
    FileKind, FileRole, ObservationSource, Severity, ShortText, TextEncoding, Timestamp,
};
use serde_json::Value;

use crate::error::RegistryError;

/// A bounded, control-free text from a stored string: control characters are dropped and the rest is cut at 256 bytes on a character
/// boundary, so a stored name or message (data the reader must show, not input to refuse) always becomes a `ShortText`. The error
/// is there for the type's sake: a text with no control characters and at most 256 bytes always parses.
pub fn short(s: &str) -> Result<ShortText, RegistryError> {
    let mut out = String::with_capacity(s.len().min(256));
    for c in s.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > 256 {
            break;
        }
        out.push(c);
    }
    ShortText::parse(&out).map_err(|_| RegistryError::Corrupt)
}

pub fn hash(bytes: &[u8]) -> Result<ContentHash, RegistryError> {
    <[u8; 32]>::try_from(bytes)
        .map(ContentHash::from_bytes)
        .map_err(|_| RegistryError::Corrupt)
}

pub fn opt_hash(bytes: Option<&[u8]>) -> Result<Option<ContentHash>, RegistryError> {
    bytes.map(hash).transpose()
}

pub fn state(s: &str) -> Result<DriftState, RegistryError> {
    s.parse().map_err(|_| RegistryError::Corrupt)
}

pub fn class(s: &str) -> Result<FileClass, RegistryError> {
    s.parse().map_err(|_| RegistryError::Corrupt)
}

pub fn kind(s: &str) -> Result<FileKind, RegistryError> {
    s.parse().map_err(|_| RegistryError::Corrupt)
}

pub fn role(s: Option<&str>) -> Result<FileRole, RegistryError> {
    // A row the pipeline has not classified yet is a resource: the role a file has until an application name says otherwise.
    s.map_or(Ok(FileRole::Resource), |s| {
        s.parse().map_err(|_| RegistryError::Corrupt)
    })
}

pub fn severity(s: Option<&str>) -> Result<Option<Severity>, RegistryError> {
    s.map(|s| s.parse().map_err(|_| RegistryError::Corrupt))
        .transpose()
}

pub fn observation_source(s: &str) -> Result<ObservationSource, RegistryError> {
    s.parse().map_err(|_| RegistryError::Corrupt)
}

pub fn timestamp(ms: Option<i64>) -> Option<Timestamp> {
    ms.map(Timestamp::from_unix_millis)
}

/// The format of a file from its three stored columns, or `None` when any is missing (the caller then reads it from the bytes).
pub fn file_format(
    eol: Option<&str>,
    encoding: Option<&str>,
    bom: Option<bool>,
) -> Result<Option<FileFormat>, RegistryError> {
    let (Some(eol), Some(encoding), Some(bom)) = (eol, encoding, bom) else {
        return Ok(None);
    };
    Ok(Some(FileFormat {
        eol: eol.parse::<EolStyle>().map_err(|_| RegistryError::Corrupt)?,
        encoding: encoding
            .parse::<TextEncoding>()
            .map_err(|_| RegistryError::Corrupt)?,
        bom,
    }))
}

/// The attribution stored on an observation. A value that does not parse is "unknown, low": showing less than was known is safe,
/// failing a whole list over one odd row is not.
pub fn attribution(source: &str, confidence: &str, actor: Option<Value>, evidence: Value) -> Attribution {
    let source: AttributionSource = source.parse().unwrap_or(AttributionSource::Unknown);
    let confidence: Confidence = confidence.parse().unwrap_or(Confidence::Low);
    Attribution {
        source,
        confidence,
        actor: actor.and_then(|v| serde_json::from_value(v).ok()),
        evidence: serde_json::from_value(evidence).unwrap_or_default(),
    }
}

/// `s` as the argument of `LIKE ... ESCAPE '\'`: the characters that mean something to `LIKE` are escaped.
pub fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The order of drift states for "the worst state under a folder" (A25), worst first, as a SQL `CASE` over a `state` column. A macro, so
/// that it can be spliced into the `const` statements with `concat!` (the EXPLAIN tests take the very text that runs).
macro_rules! state_rank_sql {
    () => {
        "CASE state WHEN 'conflict' THEN 0 WHEN 'nfs_ahead' THEN 1 WHEN 'git_ahead' THEN 2 \
         WHEN 'unknown' THEN 3 WHEN 'untracked' THEN 4 WHEN 'intentional_divergence' THEN 5 ELSE 6 END"
    };
}
pub(crate) use state_rank_sql;

/// The same, as a constant (for the test of its inverse).
pub const STATE_RANK_SQL: &str = state_rank_sql!();

/// The state with this rank (the inverse of [`STATE_RANK_SQL`]).
pub fn state_of_rank(rank: i32) -> DriftState {
    match rank {
        0 => DriftState::Conflict,
        1 => DriftState::NfsAhead,
        2 => DriftState::GitAhead,
        3 => DriftState::Unknown,
        4 => DriftState::Untracked,
        5 => DriftState::IntentionalDivergence,
        _ => DriftState::InSync,
    }
}

/// The severity rank of a stored value: low 0 to critical 3 (a SQL `CASE` for `max`).
macro_rules! severity_rank_sql {
    () => {
        "CASE severity WHEN 'low' THEN 0 WHEN 'medium' THEN 1 WHEN 'high' THEN 2 WHEN 'critical' THEN 3 END"
    };
}
pub(crate) use severity_rank_sql;

pub const SEVERITY_RANK_SQL: &str = severity_rank_sql!();

pub fn severity_of_rank(rank: Option<i32>) -> Option<Severity> {
    match rank? {
        0 => Some(Severity::Low),
        1 => Some(Severity::Medium),
        2 => Some(Severity::High),
        3 => Some(Severity::Critical),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_drops_control_characters_and_cuts_on_a_character_boundary() {
        assert_eq!(short("a\u{0}b\nc").unwrap().as_str(), "abc");
        let long = "é".repeat(200);
        let cut = short(&long).unwrap();
        assert!(cut.as_str().len() <= 256 && cut.as_str().chars().all(|c| c == 'é'));
        assert_eq!(short("").unwrap().as_str(), "");
    }

    #[test]
    fn like_patterns_are_escaped() {
        assert_eq!(like_escape("a_b%c\\d"), "a\\_b\\%c\\\\d");
        assert_eq!(like_escape("plain/dir"), "plain/dir");
    }

    #[test]
    fn the_rank_of_a_state_round_trips() {
        for (rank, s) in [
            (0, DriftState::Conflict),
            (1, DriftState::NfsAhead),
            (2, DriftState::GitAhead),
            (3, DriftState::Unknown),
            (4, DriftState::Untracked),
            (5, DriftState::IntentionalDivergence),
            (6, DriftState::InSync),
        ] {
            assert_eq!(state_of_rank(rank), s);
            assert!(
                STATE_RANK_SQL.contains(&format!("'{}' THEN {rank}", s.as_str())) || s == DriftState::InSync
            );
        }
        assert_eq!(severity_of_rank(Some(3)), Some(Severity::Critical));
        assert_eq!(severity_of_rank(None), None);
    }

    #[test]
    fn a_stored_attribution_that_does_not_parse_is_unknown_not_an_error() {
        let a = attribution(
            "nonsense",
            "nonsense",
            Some(serde_json::json!({"kind": "?"})),
            serde_json::json!(7),
        );
        assert_eq!(a, Attribution::unknown());
        let good = attribution(
            "sentinel_login",
            "high",
            Some(serde_json::json!({"kind": "os_login", "login": "ada"})),
            serde_json::json!({}),
        );
        assert_eq!(good.confidence, Confidence::High);
        assert!(good.actor.is_some());
    }

    #[test]
    fn a_hash_of_the_wrong_length_is_corrupt() {
        assert_eq!(hash(&[0; 31]), Err(RegistryError::Corrupt));
        assert!(hash(&[0; 32]).is_ok());
    }
}
