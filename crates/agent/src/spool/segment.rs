//! Segment files: the names the spool gives them, how one is read back when the spool opens, and how the active one is
//! appended to (D74).
//!
//! A segment is `seg-<16 hex digits>.lks`: the magic, then frames ([`super::record`]). The ids only grow. The spool
//! writes to one segment at a time (the *active* one) and never writes to a segment it did not create in this run, so
//! whatever damage a crash left in the old ones stays exactly where it is until they are deleted. A segment is deleted
//! as a whole, when none of its records is still owed to the hub.

use std::io::{self, BufReader, Read};

use cap_std::fs::{Dir, File, FileExt, OpenOptions, OpenOptionsExt};

use super::record::{
    FRAME_OVERHEAD, HEADER_LEN, Header, Meta, Read1, RecordError, SEGMENT_MAGIC, check_next, open_payload,
};

/// Created with this mode (before the umask): the files hold copies of config, which only the agent needs to read.
const FILE_MODE: u32 = 0o600;
/// The buffer the open-time scan reads through, and the one it checksums through.
const SCAN_BUFFER: usize = 64 * 1024;
/// The bytes of a segment with no record in it.
pub const SEGMENT_HEADER_LEN: u64 = SEGMENT_MAGIC.len() as u64;

/// The file name of segment `id`.
pub fn name(id: u64) -> String {
    format!("seg-{id:016x}.lks")
}

/// The id in a segment's file name, or `None` for any other name.
pub fn parse_name(name: &str) -> Option<u64> {
    let hex = name.strip_prefix("seg-")?.strip_suffix(".lks")?;
    if hex.len() != 16
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// A record found while scanning a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub offset: u64,
    pub frame_len: u64,
    pub meta: Meta,
}

/// Why the end of a segment could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    /// The file does not start with this build's magic.
    BadMagic,
    /// A frame is cut short or invalid. Nothing after it is read.
    Record(RecordError),
}

/// What is in a segment file.
#[derive(Debug)]
pub struct Scan {
    /// The records that were whole, in file order.
    pub found: Vec<Found>,
    /// The size of the file.
    pub file_len: u64,
    /// The end of the last whole record (the magic alone for a file with none).
    pub valid_len: u64,
    pub damage: Option<Damage>,
}

/// Read a whole segment once, checking every checksum, and stop at the first frame that is not whole. Memory is the
/// two buffers and the list of records found, whatever the size of the file.
pub fn scan(file: File) -> io::Result<Scan> {
    let file_len = file.metadata()?.len();
    if file_len == 0 {
        return Ok(Scan {
            found: Vec::new(),
            file_len,
            valid_len: 0,
            damage: None,
        });
    }
    let mut reader = BufReader::with_capacity(SCAN_BUFFER, file);
    let mut magic = [0_u8; SEGMENT_MAGIC.len()];
    let mut got = 0;
    while got < magic.len() {
        match reader.read(&mut magic[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if got < magic.len() || magic != SEGMENT_MAGIC {
        return Ok(Scan {
            found: Vec::new(),
            file_len,
            valid_len: 0,
            damage: Some(Damage::BadMagic),
        });
    }
    let mut scratch = vec![0_u8; SCAN_BUFFER];
    let mut found = Vec::new();
    let mut offset = SEGMENT_HEADER_LEN;
    loop {
        match check_next(&mut reader, &mut scratch)? {
            Read1::Frame { meta, frame_len } => {
                found.push(Found {
                    offset,
                    frame_len,
                    meta,
                });
                offset += frame_len;
            }
            Read1::End => {
                return Ok(Scan {
                    found,
                    file_len,
                    valid_len: offset,
                    damage: None,
                });
            }
            Read1::Damaged { why, .. } => {
                return Ok(Scan {
                    found,
                    file_len,
                    valid_len: offset,
                    damage: Some(Damage::Record(why)),
                });
            }
        }
    }
}

/// Why one record could not be read back for sending.
#[derive(Debug, thiserror::Error)]
pub enum ReadFailure {
    #[error("the segment could not be read: {0:?}")]
    Io(io::ErrorKind),
    /// What the index says is here is not here: the file was damaged or replaced after the spool read it.
    #[error("the record on disk is not the one the index expects: {0}")]
    Damaged(RecordError),
}

/// Read and verify the frame at `offset`. Returns the metadata and the payload (metadata and body).
pub fn read_record(file: &File, offset: u64, frame_len: u64) -> Result<(Meta, Vec<u8>), ReadFailure> {
    let io = |e: io::Error| ReadFailure::Io(e.kind());
    let mut head = [0_u8; HEADER_LEN];
    file.read_exact_at(&mut head, offset).map_err(io)?;
    let header = Header::parse(&head).map_err(ReadFailure::Damaged)?;
    if header.frame_len() != frame_len {
        return Err(ReadFailure::Damaged(RecordError::BadLength));
    }
    let mut payload = vec![0_u8; header.payload_len as usize];
    file.read_exact_at(&mut payload, offset + HEADER_LEN as u64)
        .map_err(io)?;
    let (meta, _) = open_payload(header, &payload).map_err(ReadFailure::Damaged)?;
    Ok((meta, payload))
}

/// The segment being appended to.
#[derive(Debug)]
pub struct Active {
    pub id: u64,
    file: File,
    /// The size of the file: the magic and every frame written.
    pub len: u64,
}

impl Active {
    /// Create segment `id` (it must not exist) and write the magic.
    pub fn create(dir: &Dir, id: u64) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(FILE_MODE);
        let file = dir.open_with(name(id), &options)?;
        let mut active = Self { id, file, len: 0 };
        active.write(&SEGMENT_MAGIC)?;
        Ok(active)
    }

    /// Append `bytes` (a whole frame) and return the offset it starts at. After an error the segment holds an unknown
    /// part of the frame and must not be appended to again.
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<u64> {
        debug_assert!(bytes.len() >= FRAME_OVERHEAD);
        let at = self.len;
        self.write(bytes)?;
        Ok(at)
    }

    /// Append the first `keep` bytes only: what a process killed in the middle of a write leaves behind.
    pub fn append_torn(&mut self, bytes: &[u8], keep: usize) -> io::Result<()> {
        self.write(&bytes[..keep.min(bytes.len())])
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        self.file.write_all(bytes)?;
        self.len += bytes.len() as u64;
        Ok(())
    }

    /// Flush the data (and the size) to the device.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }
}

/// Flush the directory entry of a file just created or renamed in `dir`.
pub fn sync_dir(dir: &Dir) -> io::Result<()> {
    dir.open(".")?.sync_all()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::spool::SpoolVolume;
    use crate::spool::record::Frame;

    fn meta(seq: u64) -> Meta {
        Meta {
            seq,
            part: 0,
            more: false,
            entries: 1,
            first_ms: 1,
            last_ms: 2,
        }
    }

    fn frame(seq: u64, body: &[u8]) -> Frame {
        let mut buf = Frame::begin(body.len());
        buf.extend_from_slice(body);
        Frame::seal(buf, &meta(seq)).unwrap()
    }

    #[test]
    fn names_round_trip_and_nothing_else_parses() {
        for id in [0, 1, 0xdead_beef, u64::MAX] {
            assert_eq!(parse_name(&name(id)), Some(id));
        }
        for bad in [
            "state",
            "seg-1.lks",
            "seg-0000000000000001.lks.tmp",
            "seg-000000000000000G.lks",
            "seg-000000000000000A.lks",
            "../seg-0000000000000001.lks",
            "seg-+000000000000001.lks",
            "",
        ] {
            assert_eq!(parse_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn what_is_appended_is_found_again_with_its_offset() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        let mut active = Active::create(volume.dir(), 7).unwrap();
        let (a, b) = (frame(1, b"first"), frame(2, b"second, longer"));
        let at_a = active.append(a.as_bytes()).unwrap();
        let at_b = active.append(b.as_bytes()).unwrap();
        active.sync().unwrap();
        assert_eq!(
            (at_a, at_b),
            (SEGMENT_HEADER_LEN, SEGMENT_HEADER_LEN + a.len() as u64)
        );

        let scan = scan(volume.dir().open(name(7)).unwrap()).unwrap();
        assert_eq!(scan.damage, None);
        assert_eq!(scan.valid_len, scan.file_len);
        assert_eq!(scan.found.iter().map(|f| f.meta.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(scan.found[1].offset, at_b);

        let file = volume.dir().open(name(7)).unwrap();
        let (m, payload) = read_record(&file, at_b, scan.found[1].frame_len).unwrap();
        assert_eq!(m.seq, 2);
        assert!(payload.ends_with(b"second, longer"));
        // The index and the file must agree on the size, or the read is refused.
        assert!(matches!(
            read_record(&file, at_b, scan.found[1].frame_len + 1),
            Err(ReadFailure::Damaged(RecordError::BadLength))
        ));
    }

    #[test]
    fn a_segment_is_never_created_twice() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        Active::create(volume.dir(), 3).unwrap();
        assert_eq!(
            Active::create(volume.dir(), 3).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn a_torn_frame_ends_the_scan_at_the_last_whole_record() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        let mut active = Active::create(volume.dir(), 1).unwrap();
        active.append(frame(1, b"whole").as_bytes()).unwrap();
        let end_of_first = active.len;
        let torn = frame(2, b"torn in half");
        active.append_torn(torn.as_bytes(), 20).unwrap();
        let scan = scan(volume.dir().open(name(1)).unwrap()).unwrap();
        assert_eq!(scan.found.len(), 1);
        assert_eq!(scan.valid_len, end_of_first);
        assert_eq!(scan.file_len, end_of_first + 20);
        assert_eq!(scan.damage, Some(Damage::Record(RecordError::Truncated)));
    }

    #[test]
    fn a_file_that_is_not_a_segment_is_damage_and_an_empty_one_is_not() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        volume.dir().write(name(1), b"").unwrap();
        volume.dir().write(name(2), b"LKSPOOL").unwrap();
        volume.dir().write(name(3), b"NOTSPOOLxxxxxxxxxxxxxxxx").unwrap();
        let of = |id| scan(volume.dir().open(name(id)).unwrap()).unwrap();
        assert_eq!(of(1).damage, None);
        assert_eq!(of(2).damage, Some(Damage::BadMagic));
        assert_eq!(of(3).damage, Some(Damage::BadMagic));
        assert!(of(3).found.is_empty());
    }

    #[test]
    fn a_segment_is_created_private() {
        use cap_std::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        Active::create(volume.dir(), 1).unwrap();
        let mode = volume.dir().metadata(name(1)).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
}
