//! A strict `multipart/form-data` reader for the upload and preview routes (S11, S22).
//!
//! It reads a body that is already in memory and already under its size limit, and accepts only what the two forms of the
//! contract send: the fields `action`, `path`, `expected_state`, `expected_hash`, `preview_id` and `file`, each at most once, each
//! part with a `Content-Disposition: form-data; name="..."` header (a `filename` is allowed and ignored) and optionally a
//! `Content-Type`. Everything else is refused: a preamble, another header, a field the contract does not name, a field twice, a
//! delimiter that is not followed by a line end, a part that never ends. No error says where the body went wrong or quotes any of it.
//!
//! Work is linear in the size of the body: a part's header block is at most [`MAX_HEADER_BYTES`], a text field at most
//! [`MAX_TEXT_BYTES`], a body at most [`MAX_PARTS`] parts, and the delimiter search never looks at a byte twice more than the
//! delimiter is long.

use bytes::Bytes;

/// The fields of the two forms.
pub const FIELDS: [&str; 6] = [
    "action",
    "path",
    "expected_state",
    "expected_hash",
    "preview_id",
    "file",
];

/// A part's header block, in bytes.
pub const MAX_HEADER_BYTES: usize = 2 * 1024;
/// A text field's value, in bytes. A path is at most 1,024.
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
/// The most parts: one for each field.
pub const MAX_PARTS: usize = FIELDS.len();
/// RFC 2046: a boundary is 1 to 70 characters.
const MAX_BOUNDARY: usize = 70;

/// Why a body was refused. Nothing in it comes from the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MultipartError {
    #[error("the body is not multipart/form-data")]
    NotMultipart,
    #[error("the multipart body is malformed")]
    Malformed,
    #[error("the form has a field the contract does not name")]
    UnknownField,
    #[error("the form has a field twice")]
    Duplicate,
    #[error("a field is larger than its limit")]
    TooLarge,
}

/// The fields of a form that was read.
#[derive(Debug, Default)]
pub struct Form {
    fields: [Option<Bytes>; FIELDS.len()],
}

impl Form {
    fn index(name: &str) -> Option<usize> {
        FIELDS.iter().position(|f| *f == name)
    }

    /// A text field's value, if it was sent and is UTF-8.
    pub fn text(&self, name: &str) -> Option<Result<&str, MultipartError>> {
        let value = self.fields.get(Self::index(name)?)?.as_ref()?;
        Some(std::str::from_utf8(value).map_err(|_| MultipartError::Malformed))
    }

    /// The file, if it was sent.
    pub fn file(&self) -> Option<Bytes> {
        self.fields.get(Self::index("file")?)?.clone()
    }

    pub fn has(&self, name: &str) -> bool {
        Self::index(name)
            .and_then(|i| self.fields.get(i))
            .is_some_and(Option::is_some)
    }
}

/// The boundary of a `multipart/form-data` content type.
fn boundary(content_type: &str) -> Result<&str, MultipartError> {
    let mut parts = content_type.split(';');
    let media = parts.next().unwrap_or("").trim();
    if !media.eq_ignore_ascii_case("multipart/form-data") {
        return Err(MultipartError::NotMultipart);
    }
    let mut found = None;
    for param in parts {
        let Some((key, value)) = param.split_once('=') else {
            return Err(MultipartError::Malformed);
        };
        if key.trim().eq_ignore_ascii_case("boundary") {
            if found.is_some() {
                return Err(MultipartError::Malformed);
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            found = Some(value);
        }
    }
    let boundary = found.ok_or(MultipartError::Malformed)?;
    let valid_char = |c: char| c.is_ascii_alphanumeric() || "'()+_,-./:=? ".contains(c);
    if boundary.is_empty()
        || boundary.len() > MAX_BOUNDARY
        || boundary.ends_with(' ')
        || !boundary.chars().all(valid_char)
    {
        return Err(MultipartError::Malformed);
    }
    Ok(boundary)
}

/// The first place `needle` starts in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut from = 0;
    while let Some(i) = hay.get(from..)?.iter().position(|&b| b == first) {
        let at = from + i;
        if hay.get(at..)?.starts_with(needle) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// The field a part's headers name.
fn field_name(headers: &[u8]) -> Result<usize, MultipartError> {
    let text = std::str::from_utf8(headers).map_err(|_| MultipartError::Malformed)?;
    if !text.is_ascii() {
        return Err(MultipartError::Malformed);
    }
    let mut name = None;
    let mut seen_disposition = false;
    for line in text.split("\r\n") {
        // A folded header (a line that starts with whitespace) is not something a browser sends.
        if line.starts_with([' ', '\t']) || line.is_empty() {
            return Err(MultipartError::Malformed);
        }
        let (key, value) = line.split_once(':').ok_or(MultipartError::Malformed)?;
        match key.trim().to_ascii_lowercase().as_str() {
            "content-disposition" => {
                if seen_disposition {
                    return Err(MultipartError::Malformed);
                }
                seen_disposition = true;
                name = Some(disposition_name(value)?);
            }
            "content-type" => {}
            _ => return Err(MultipartError::Malformed),
        }
    }
    let name = name.ok_or(MultipartError::Malformed)?;
    Form::index(&name).ok_or(MultipartError::UnknownField)
}

/// The `name` of a `Content-Disposition: form-data; name="x"; filename="y"` value.
fn disposition_name(value: &str) -> Result<String, MultipartError> {
    let mut params = value.split(';');
    if !params
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("form-data")
    {
        return Err(MultipartError::Malformed);
    }
    let mut name = None;
    let mut filename = false;
    for param in params {
        let (key, raw) = param.split_once('=').ok_or(MultipartError::Malformed)?;
        let raw = raw.trim();
        let quoted = raw
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or(MultipartError::Malformed)?;
        // No escapes: a name or a file name with a quote or a backslash in it is not one the contract allows.
        if quoted.contains(['"', '\\', '\r', '\n']) {
            return Err(MultipartError::Malformed);
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "name" => {
                if name.replace(quoted.to_owned()).is_some() {
                    return Err(MultipartError::Malformed);
                }
            }
            "filename" => {
                if std::mem::replace(&mut filename, true) {
                    return Err(MultipartError::Malformed);
                }
            }
            _ => return Err(MultipartError::Malformed),
        }
    }
    name.ok_or(MultipartError::Malformed)
}

/// Read a multipart body. `max_file` is the most the `file` field may hold.
pub fn parse(content_type: &str, body: &Bytes, max_file: usize) -> Result<Form, MultipartError> {
    let boundary = boundary(content_type)?;
    let mut delimiter = Vec::with_capacity(boundary.len() + 2);
    delimiter.extend_from_slice(b"--");
    delimiter.extend_from_slice(boundary.as_bytes());
    // What ends a part's data: a line end, then the delimiter.
    let mut closing = Vec::with_capacity(delimiter.len() + 2);
    closing.extend_from_slice(b"\r\n");
    closing.extend_from_slice(&delimiter);

    // No preamble: the body starts with the first delimiter.
    if !body.starts_with(&delimiter) {
        return Err(MultipartError::Malformed);
    }
    let mut pos = delimiter.len();
    let mut form = Form::default();
    let mut parts = 0_usize;
    loop {
        let rest = body.get(pos..).ok_or(MultipartError::Malformed)?;
        if let Some(tail) = rest.strip_prefix(b"--") {
            // The closing delimiter: nothing may follow it but a line end.
            return if tail.is_empty() || tail == b"\r\n" {
                Ok(form)
            } else {
                Err(MultipartError::Malformed)
            };
        }
        let rest = rest.strip_prefix(b"\r\n").ok_or(MultipartError::Malformed)?;
        pos += 2;

        parts += 1;
        if parts > MAX_PARTS {
            return Err(MultipartError::TooLarge);
        }
        let header_len = find(
            rest.get(..rest.len().min(MAX_HEADER_BYTES + 4)).unwrap_or(rest),
            b"\r\n\r\n",
        )
        .ok_or(MultipartError::Malformed)?;
        let index = field_name(rest.get(..header_len).ok_or(MultipartError::Malformed)?)?;
        pos += header_len + 4;

        let data_len = find(body.get(pos..).ok_or(MultipartError::Malformed)?, &closing)
            .ok_or(MultipartError::Malformed)?;
        let limit = if FIELDS.get(index) == Some(&"file") {
            max_file
        } else {
            MAX_TEXT_BYTES
        };
        if data_len > limit {
            return Err(MultipartError::TooLarge);
        }
        let slot = form.fields.get_mut(index).ok_or(MultipartError::Malformed)?;
        if slot.is_some() {
            return Err(MultipartError::Duplicate);
        }
        *slot = Some(body.slice(pos..pos + data_len));
        pos += data_len + closing.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CT: &str = "multipart/form-data; boundary=XyZ";

    fn part(name: &str, data: &[u8]) -> Vec<u8> {
        let mut out =
            format!("--XyZ\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n").into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
        out
    }

    fn body(parts: &[Vec<u8>]) -> Bytes {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(p);
        }
        out.extend_from_slice(b"--XyZ--\r\n");
        Bytes::from(out)
    }

    #[test]
    fn a_form_is_read_field_by_field() {
        let b = body(&[
            part("path", b"a/b.yml"),
            part("file", b"x: 1\r\n\r\n--not a boundary\n"),
        ]);
        let form = parse(CT, &b, 1024).unwrap();
        assert_eq!(form.text("path").unwrap().unwrap(), "a/b.yml");
        assert_eq!(form.file().unwrap().as_ref(), b"x: 1\r\n\r\n--not a boundary\n");
        assert!(!form.has("action"));
    }

    #[test]
    fn a_file_may_hold_any_bytes_but_the_delimiter() {
        let data: Vec<u8> = (0..=255).collect();
        let b = body(&[part("file", &data)]);
        assert_eq!(parse(CT, &b, 1024).unwrap().file().unwrap().as_ref(), &data[..]);
        // An empty file is a file.
        let b = body(&[part("file", b"")]);
        assert_eq!(parse(CT, &b, 1024).unwrap().file().unwrap().len(), 0);
    }

    #[test]
    fn a_file_name_is_allowed_and_ignored() {
        let b = Bytes::from_static(
            b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"logo.png\"\r\nContent-Type: image/png\r\n\r\nPNG\r\n--XyZ--\r\n",
        );
        assert_eq!(parse(CT, &b, 1024).unwrap().file().unwrap().as_ref(), b"PNG");
    }

    #[test]
    fn what_the_contract_does_not_name_is_refused() {
        let unknown = body(&[part("extra", b"1")]);
        assert_eq!(
            parse(CT, &unknown, 1024).unwrap_err(),
            MultipartError::UnknownField
        );
        let twice = body(&[part("path", b"a"), part("path", b"b")]);
        assert_eq!(parse(CT, &twice, 1024).unwrap_err(), MultipartError::Duplicate);
        let big = body(&[part("file", &[b'x'; 11])]);
        assert_eq!(parse(CT, &big, 10).unwrap_err(), MultipartError::TooLarge);
        assert!(
            parse(CT, &body(&[part("file", &[b'x'; 10])]), 10).is_ok(),
            "exactly at the limit"
        );
        let long_text = body(&[part("path", &vec![b'a'; MAX_TEXT_BYTES + 1])]);
        assert_eq!(parse(CT, &long_text, 1024).unwrap_err(), MultipartError::TooLarge);
    }

    #[test]
    fn a_body_that_is_not_exactly_a_form_is_refused() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", b"".to_vec()),
            (
                "preamble",
                [b"junk\r\n".as_slice(), &body(&[part("path", b"a")])].concat(),
            ),
            ("no closing delimiter", part("path", b"a")),
            (
                "data after the closing delimiter",
                [body(&[part("path", b"a")]).to_vec(), b"x".to_vec()].concat(),
            ),
            (
                "part never ends",
                b"--XyZ\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\nabc".to_vec(),
            ),
            (
                "no header block end",
                b"--XyZ\r\nContent-Disposition: form-data; name=\"path\"\r\n".to_vec(),
            ),
            ("delimiter not followed by a line end", b"--XyZxx\r\n".to_vec()),
            (
                "other header",
                b"--XyZ\r\nX-Other: 1\r\nContent-Disposition: form-data; name=\"path\"\r\n\r\na\r\n--XyZ--"
                    .to_vec(),
            ),
            (
                "no disposition",
                b"--XyZ\r\nContent-Type: text/plain\r\n\r\na\r\n--XyZ--".to_vec(),
            ),
            (
                "not form-data",
                b"--XyZ\r\nContent-Disposition: attachment; name=\"path\"\r\n\r\na\r\n--XyZ--".to_vec(),
            ),
            (
                "unquoted name",
                b"--XyZ\r\nContent-Disposition: form-data; name=path\r\n\r\na\r\n--XyZ--".to_vec(),
            ),
            (
                "escaped quote",
                b"--XyZ\r\nContent-Disposition: form-data; name=\"pa\\\"th\"\r\n\r\na\r\n--XyZ--".to_vec(),
            ),
            (
                "folded header",
                b"--XyZ\r\nContent-Disposition: form-data;\r\n name=\"path\"\r\n\r\na\r\n--XyZ--".to_vec(),
            ),
            (
                "name twice",
                b"--XyZ\r\nContent-Disposition: form-data; name=\"path\"; name=\"file\"\r\n\r\na\r\n--XyZ--"
                    .to_vec(),
            ),
            (
                "non-ascii header",
                "--XyZ\r\nContent-Disposition: form-data; name=\"pa\u{e9}\"\r\n\r\na\r\n--XyZ--"
                    .as_bytes()
                    .to_vec(),
            ),
            ("lone delimiter", b"--XyZ".to_vec()),
        ];
        for (what, bytes) in cases {
            assert!(parse(CT, &Bytes::from(bytes), 1024).is_err(), "{what}");
        }
    }

    #[test]
    fn too_many_parts_are_refused() {
        let many: Vec<Vec<u8>> = (0..=MAX_PARTS).map(|_| part("path", b"a")).collect();
        assert!(parse(CT, &body(&many), 1024).is_err());
    }

    #[test]
    fn the_content_type_has_to_say_multipart_with_a_boundary() {
        let b = body(&[part("path", b"a")]);
        for ct in [
            "application/json",
            "multipart/form-data",
            "multipart/form-data; boundary=",
            "multipart/form-data; boundary=a; boundary=b",
            "multipart/form-data; boundary=\"unterminated",
            &format!("multipart/form-data; boundary={}", "a".repeat(71)),
            "multipart/form-data; boundary=bad\u{1}char",
            "multipart/mixed; boundary=XyZ",
        ] {
            assert!(parse(ct, &b, 1024).is_err(), "{ct}");
        }
        assert!(parse("Multipart/Form-Data; Boundary=\"XyZ\"", &b, 1024).is_ok());
    }

    #[test]
    fn errors_say_nothing_of_the_body() {
        for e in [
            MultipartError::NotMultipart,
            MultipartError::Malformed,
            MultipartError::UnknownField,
            MultipartError::Duplicate,
            MultipartError::TooLarge,
        ] {
            assert!(!format!("{e} {e:?}").contains("XyZ"));
        }
    }
}
