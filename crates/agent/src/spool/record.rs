//! One record of the spool, and how it is framed on disk (D74, S22).
//!
//! A segment file is the 8-byte magic [`SEGMENT_MAGIC`] followed by frames:
//!
//! ```text
//! frame   = payload_len u32 LE | crc32 u32 LE | payload            (the checksum covers the payload)
//! payload = meta (36 bytes)    | body                              (body = one delta message, protobuf)
//! meta    = version u8 | flags u8 | 0 u16 | part u32 | seq u64 | first_ms i64 | last_ms i64 | entries u32
//! ```
//!
//! - The **body** is the same protobuf message the hub gets (`AgentMessage` with a `ScanDelta`), so a spooled version
//!   carries its path, hash, bytes, `observed_at` and the `denied` flag exactly as they would be sent. A denied entry
//!   has no bytes on the wire, and so none in the spool (D79).
//! - The **meta** repeats what the spool must know without decoding the body: the order (`seq`, `part`, `more`), how many
//!   entries the record holds (the bound counts them) and when they were seen (a gap reports the range). Opening a
//!   spool reads every byte once to check the checksums, and decodes nothing.
//! - The **length** is checked against [`MAX_PAYLOAD`] before anything is read or allocated, so a damaged length cannot
//!   make the spool read or allocate gigabytes.
//!
//! A frame that is cut short, or whose checksum does not match, ends its segment: nothing after it can be framed with
//! any confidence, and the spool says how much it could not read instead of guessing.

use std::io::{self, Read};

/// The first eight bytes of every segment. The last byte is the format version.
pub const SEGMENT_MAGIC: [u8; 8] = *b"LKSPOOL1";
/// Bytes before the payload: its length and its checksum.
pub const HEADER_LEN: usize = 8;
/// Bytes of metadata at the start of the payload.
pub const META_LEN: usize = 36;
/// What a frame adds to its body.
pub const FRAME_OVERHEAD: usize = HEADER_LEN + META_LEN;
/// The largest body: one wire message.
pub const MAX_BODY: usize = proto::limits::MAX_MESSAGE_BYTES;
/// The largest payload a frame may claim.
pub const MAX_PAYLOAD: usize = META_LEN + MAX_BODY;

const META_VERSION: u8 = 1;
const FLAG_MORE: u8 = 0b0000_0001;

/// What the spool knows about a record without decoding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub seq: u64,
    /// Zero-based index of the message within its logical delta.
    pub part: u32,
    /// More messages of the same logical delta follow.
    pub more: bool,
    /// Entries, removals and skips in the body: what the spool's entry bound counts.
    pub entries: u32,
    /// When the earliest and latest version in the body were seen (Unix milliseconds), `first_ms <= last_ms`.
    pub first_ms: i64,
    pub last_ms: i64,
}

impl Meta {
    fn encode(&self) -> [u8; META_LEN] {
        let mut out = [0_u8; META_LEN];
        out[0] = META_VERSION;
        out[1] = if self.more { FLAG_MORE } else { 0 };
        out[4..8].copy_from_slice(&self.part.to_le_bytes());
        out[8..16].copy_from_slice(&self.seq.to_le_bytes());
        out[16..24].copy_from_slice(&self.first_ms.to_le_bytes());
        out[24..32].copy_from_slice(&self.last_ms.to_le_bytes());
        out[32..36].copy_from_slice(&self.entries.to_le_bytes());
        out
    }

    /// `None` for an unknown version, unknown flags or nonzero reserved bytes: not something this build wrote.
    fn decode(bytes: &[u8; META_LEN]) -> Option<Self> {
        if bytes[0] != META_VERSION || bytes[1] & !FLAG_MORE != 0 || bytes[2] != 0 || bytes[3] != 0 {
            return None;
        }
        let first_ms = i64::from_le_bytes(bytes[16..24].try_into().ok()?);
        let last_ms = i64::from_le_bytes(bytes[24..32].try_into().ok()?);
        if first_ms > last_ms {
            return None;
        }
        Some(Self {
            more: bytes[1] & FLAG_MORE != 0,
            part: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            seq: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            first_ms,
            last_ms,
            entries: u32::from_le_bytes(bytes[32..36].try_into().ok()?),
        })
    }
}

/// Why a frame could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// The body is bigger than any wire message can be.
    #[error("the record body is larger than a wire message")]
    TooLarge,
    /// The file ended inside a frame.
    #[error("the file ends inside a record")]
    Truncated,
    /// The length field is zero, shorter than the metadata, or larger than any record can be.
    #[error("a record claims an impossible length")]
    BadLength,
    /// The checksum does not match the bytes.
    #[error("a record fails its checksum")]
    BadChecksum,
    /// The checksum matched but the metadata is not something this build writes.
    #[error("a record has metadata this build does not understand")]
    BadMeta,
}

/// A frame ready to be written: header, metadata and body in one buffer.
#[derive(Debug)]
pub struct Frame(Vec<u8>);

impl Frame {
    /// An empty buffer with room for the header and metadata at the front. Encode the body after it (`extend`,
    /// `Message::encode`), then call [`Frame::seal`].
    pub fn begin(body_hint: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(FRAME_OVERHEAD + body_hint);
        buf.resize(FRAME_OVERHEAD, 0);
        buf
    }

    /// Fill in the length, the metadata and the checksum of a buffer made by [`Frame::begin`].
    pub fn seal(mut buf: Vec<u8>, meta: &Meta) -> Result<Self, RecordError> {
        if buf.len() < FRAME_OVERHEAD || buf.len() - FRAME_OVERHEAD > MAX_BODY {
            return Err(RecordError::TooLarge);
        }
        let payload_len = u32::try_from(buf.len() - HEADER_LEN).map_err(|_| RecordError::TooLarge)?;
        buf[HEADER_LEN..FRAME_OVERHEAD].copy_from_slice(&meta.encode());
        let crc = crc32fast::hash(&buf[HEADER_LEN..]);
        buf[0..4].copy_from_slice(&payload_len.to_le_bytes());
        buf[4..8].copy_from_slice(&crc.to_le_bytes());
        Ok(Self(buf))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The length and checksum at the front of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub payload_len: u32,
    pub crc: u32,
}

impl Header {
    pub fn parse(bytes: &[u8; HEADER_LEN]) -> Result<Self, RecordError> {
        let payload_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let crc = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let len = payload_len as usize;
        if !(META_LEN..=MAX_PAYLOAD).contains(&len) {
            return Err(RecordError::BadLength);
        }
        Ok(Self { payload_len, crc })
    }

    /// The size of the whole frame.
    pub fn frame_len(&self) -> u64 {
        HEADER_LEN as u64 + u64::from(self.payload_len)
    }
}

/// A verified payload: the metadata and the body that follows it.
pub fn open_payload(header: Header, payload: &[u8]) -> Result<(Meta, &[u8]), RecordError> {
    if payload.len() != header.payload_len as usize || payload.len() < META_LEN {
        return Err(RecordError::Truncated);
    }
    if crc32fast::hash(payload) != header.crc {
        return Err(RecordError::BadChecksum);
    }
    let meta: &[u8; META_LEN] = payload[..META_LEN].try_into().map_err(|_| RecordError::BadMeta)?;
    let meta = Meta::decode(meta).ok_or(RecordError::BadMeta)?;
    Ok((meta, &payload[META_LEN..]))
}

/// What reading one frame from the front of a stream found.
#[derive(Debug, PartialEq, Eq)]
pub enum Read1 {
    /// A whole frame whose checksum matched: its metadata and its size on disk.
    Frame { meta: Meta, frame_len: u64 },
    /// The stream ended exactly between frames.
    End,
    /// The stream ended inside a frame, or the frame is not valid. `consumed` bytes of it were read.
    Damaged { why: RecordError, consumed: u64 },
}

/// Read one frame from `reader` and check it, without holding its body: the checksum is computed over a small buffer
/// that is reused, so opening a spool of any size costs a fixed amount of memory. `scratch` must not be empty.
pub fn check_next<R: Read>(reader: &mut R, scratch: &mut [u8]) -> io::Result<Read1> {
    let mut head = [0_u8; HEADER_LEN];
    let got = fill(reader, &mut head)?;
    if got == 0 {
        return Ok(Read1::End);
    }
    if got < HEADER_LEN {
        return Ok(Read1::Damaged {
            why: RecordError::Truncated,
            consumed: got as u64,
        });
    }
    let header = match Header::parse(&head) {
        Ok(header) => header,
        Err(why) => {
            return Ok(Read1::Damaged {
                why,
                consumed: HEADER_LEN as u64,
            });
        }
    };
    let mut meta_bytes = [0_u8; META_LEN];
    let got_meta = fill(reader, &mut meta_bytes)?;
    if got_meta < META_LEN {
        return Ok(Read1::Damaged {
            why: RecordError::Truncated,
            consumed: (HEADER_LEN + got_meta) as u64,
        });
    }
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&meta_bytes);
    let mut left = header.payload_len as usize - META_LEN;
    let mut consumed = (HEADER_LEN + META_LEN) as u64;
    while left > 0 {
        let want = left.min(scratch.len());
        let got = fill(reader, &mut scratch[..want])?;
        hasher.update(&scratch[..got]);
        consumed += got as u64;
        if got < want {
            return Ok(Read1::Damaged {
                why: RecordError::Truncated,
                consumed,
            });
        }
        left -= got;
    }
    if hasher.finalize() != header.crc {
        return Ok(Read1::Damaged {
            why: RecordError::BadChecksum,
            consumed,
        });
    }
    match Meta::decode(&meta_bytes) {
        Some(meta) => Ok(Read1::Frame {
            meta,
            frame_len: header.frame_len(),
        }),
        None => Ok(Read1::Damaged {
            why: RecordError::BadMeta,
            consumed,
        }),
    }
}

/// Read until `buf` is full or the stream ends; returns how many bytes arrived.
fn fill<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use proptest::prelude::*;

    use super::*;

    fn meta(seq: u64) -> Meta {
        Meta {
            seq,
            part: 2,
            more: true,
            entries: 7,
            first_ms: -5,
            last_ms: 1_700_000_000_123,
        }
    }

    fn frame(seq: u64, body: &[u8]) -> Frame {
        let mut buf = Frame::begin(body.len());
        buf.extend_from_slice(body);
        Frame::seal(buf, &meta(seq)).unwrap()
    }

    #[test]
    fn a_frame_round_trips_through_the_streaming_check_and_the_payload_check() {
        let f = frame(9, b"hello body");
        let mut scratch = [0_u8; 4];
        let mut reader = Cursor::new(f.as_bytes().to_vec());
        match check_next(&mut reader, &mut scratch).unwrap() {
            Read1::Frame { meta: m, frame_len } => {
                assert_eq!(m, meta(9));
                assert_eq!(frame_len, f.len() as u64);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(check_next(&mut reader, &mut scratch).unwrap(), Read1::End);

        let head: [u8; HEADER_LEN] = f.as_bytes()[..HEADER_LEN].try_into().unwrap();
        let header = Header::parse(&head).unwrap();
        let (m, body) = open_payload(header, &f.as_bytes()[HEADER_LEN..]).unwrap();
        assert_eq!(m, meta(9));
        assert_eq!(body, b"hello body");
    }

    #[test]
    fn a_body_over_one_wire_message_is_refused() {
        let mut buf = Frame::begin(0);
        buf.resize(FRAME_OVERHEAD + MAX_BODY + 1, 0);
        assert_eq!(Frame::seal(buf, &meta(1)).unwrap_err(), RecordError::TooLarge);
        let mut buf = Frame::begin(0);
        buf.resize(FRAME_OVERHEAD + MAX_BODY, 0);
        assert!(Frame::seal(buf, &meta(1)).is_ok());
    }

    #[test]
    fn an_impossible_length_is_refused_before_anything_is_read() {
        for len in [0_u32, 1, 35, 4 * 1024 * 1024 + 36 + 1, u32::MAX] {
            let mut head = [0_u8; HEADER_LEN];
            head[..4].copy_from_slice(&len.to_le_bytes());
            assert_eq!(Header::parse(&head).unwrap_err(), RecordError::BadLength, "{len}");
        }
        // Through the stream: the claimed gigabytes are never read, and the damage is reported at once.
        let mut bytes = vec![0xFF_u8; HEADER_LEN];
        bytes.extend_from_slice(&[0_u8; 64]);
        let mut scratch = [0_u8; 16];
        assert_eq!(
            check_next(&mut Cursor::new(bytes), &mut scratch).unwrap(),
            Read1::Damaged {
                why: RecordError::BadLength,
                consumed: HEADER_LEN as u64
            }
        );
    }

    #[test]
    fn metadata_this_build_does_not_write_is_refused_even_with_a_good_checksum() {
        for tamper in [
            |b: &mut [u8]| b[HEADER_LEN] = 2,
            |b: &mut [u8]| b[HEADER_LEN + 1] = 0b10,
            |b: &mut [u8]| b[HEADER_LEN + 2] = 1,
            |b: &mut [u8]| b[HEADER_LEN + 16..HEADER_LEN + 24].copy_from_slice(&i64::MAX.to_le_bytes()),
        ] {
            let mut bytes = frame(1, b"x").as_bytes().to_vec();
            tamper(&mut bytes);
            let crc = crc32fast::hash(&bytes[HEADER_LEN..]);
            bytes[4..8].copy_from_slice(&crc.to_le_bytes());
            let mut scratch = [0_u8; 8];
            assert!(matches!(
                check_next(&mut Cursor::new(bytes), &mut scratch).unwrap(),
                Read1::Damaged {
                    why: RecordError::BadMeta,
                    ..
                }
            ));
        }
    }

    proptest! {
        /// Every proper prefix of a frame is reported as damage (or as the clean end for the empty prefix), never as a
        /// frame, and a flipped bit anywhere is never accepted.
        #[test]
        fn truncation_and_bit_flips_are_always_noticed(
            body in proptest::collection::vec(any::<u8>(), 0..300),
            cut in 0_usize..400,
            flip in 0_usize..400,
            bit in 0_u8..8,
        ) {
            let f = frame(5, &body);
            let bytes = f.as_bytes();
            let mut scratch = [0_u8; 32];
            let cut = cut % bytes.len();
            let outcome = check_next(&mut Cursor::new(bytes[..cut].to_vec()), &mut scratch).unwrap();
            if cut == 0 {
                prop_assert_eq!(outcome, Read1::End);
            } else {
                prop_assert!(matches!(outcome, Read1::Damaged { .. }), "{outcome:?}");
            }
            let mut flipped = bytes.to_vec();
            let at = flip % flipped.len();
            flipped[at] ^= 1 << bit;
            let outcome = check_next(&mut Cursor::new(flipped), &mut scratch).unwrap();
            prop_assert!(matches!(outcome, Read1::Damaged { .. }), "flip at {at}: {outcome:?}");
        }
    }
}
