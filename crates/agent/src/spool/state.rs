//! The facts the segments cannot hold (D74): how far the sequence numbers have been reserved, which records are already
//! settled, and what the hub is still owed for records that were dropped.
//!
//! - **Reserved sequence.** Every message the agent sends takes a number from one counter, including the answers to the
//!   hub's own requests, which are never spooled. After a restart the counter must start above every number ever used,
//!   so the file holds a number the counter has not reached yet; it is raised in steps ahead of the counter, and a
//!   restart starts at it. Numbers are skipped after a restart, never reused.
//! - **The floor.** Every record below this number was acknowledged or dropped. A segment is deleted only when all of
//!   its records are settled, so the active segment still holds records the hub has long acknowledged, and one that was
//!   dropped to make room is still in the file; without the floor a restart would send the acknowledged ones again and
//!   count the dropped ones as lost a second time. The floor moves up with every drop (it is written with the gap) and
//!   with acknowledgements at most once a second, so after a restart at most the last second of acknowledgements is
//!   sent again.
//! - **The pending gap.** A loss is written down the moment it happens, so a crash before the hub heard of it does not
//!   forget it.
//!
//! The file is small and replaced whole: written to `state.tmp`, synced, renamed over `state`, and the directory synced,
//! so a crash leaves the old file or the new one. A file that fails its checksum is treated as missing, and the counter
//! then starts a whole step above anything in the segments.

use std::io::{self, Read, Write};

use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use domain::{SpoolGap, Timestamp};

use super::segment::sync_dir;

pub const STATE_FILE: &str = "state";
pub const STATE_TMP: &str = "state.tmp";

const MAGIC: [u8; 8] = *b"LKSPST01";
const LEN: usize = 60;
const FILE_MODE: u32 = 0o600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// The counter may be anywhere below this; nothing at or above it has been handed out.
    pub reserved_seq: u64,
    /// Records with a sequence number below this are settled: acknowledged, dropped or discarded.
    pub floor_seq: u64,
    pub gap: Option<SpoolGap>,
}

/// What reading the state file found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loaded {
    Missing,
    /// Present but not something this build wrote, or damaged.
    Invalid,
    Valid(State),
}

fn encode(state: &State) -> [u8; LEN] {
    let mut out = [0_u8; LEN];
    out[..8].copy_from_slice(&MAGIC);
    out[8..16].copy_from_slice(&state.reserved_seq.to_le_bytes());
    out[16..24].copy_from_slice(&state.floor_seq.to_le_bytes());
    if let Some(gap) = state.gap {
        out[24] = 1;
        out[32..40].copy_from_slice(&gap.from.unix_millis().to_le_bytes());
        out[40..48].copy_from_slice(&gap.to.unix_millis().to_le_bytes());
        out[48..56].copy_from_slice(&gap.lost_entries.to_le_bytes());
    }
    let crc = crc32fast::hash(&out[..56]);
    out[56..60].copy_from_slice(&crc.to_le_bytes());
    out
}

fn decode(bytes: &[u8]) -> Option<State> {
    let bytes: &[u8; LEN] = bytes.try_into().ok()?;
    if bytes[..8] != MAGIC
        || crc32fast::hash(&bytes[..56]) != u32::from_le_bytes(bytes[56..60].try_into().ok()?)
    {
        return None;
    }
    let reserved_seq = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let floor_seq = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let gap = match bytes[24] {
        0 => None,
        1 => {
            let from = i64::from_le_bytes(bytes[32..40].try_into().ok()?);
            let to = i64::from_le_bytes(bytes[40..48].try_into().ok()?);
            if from > to {
                return None;
            }
            Some(SpoolGap {
                from: Timestamp::from_unix_millis(from),
                to: Timestamp::from_unix_millis(to),
                lost_entries: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
            })
        }
        _ => return None,
    };
    Some(State {
        reserved_seq,
        floor_seq,
        gap,
    })
}

/// Read the state file.
pub fn load(dir: &Dir) -> io::Result<Loaded> {
    let mut file = match dir.open(STATE_FILE) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(e) => return Err(e),
    };
    // One byte more than the file should have, so a file that is too long is noticed.
    let mut bytes = Vec::with_capacity(LEN + 1);
    (&mut file).take(LEN as u64 + 1).read_to_end(&mut bytes)?;
    Ok(decode(&bytes).map_or(Loaded::Invalid, Loaded::Valid))
}

/// Replace the state file.
pub fn store(dir: &Dir, state: &State) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(FILE_MODE);
    let mut file = dir.open_with(STATE_TMP, &options)?;
    file.write_all(&encode(state))?;
    file.sync_all()?;
    drop(file);
    dir.rename(STATE_TMP, dir, STATE_FILE)?;
    sync_dir(dir)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::spool::SpoolVolume;

    fn gap() -> SpoolGap {
        SpoolGap {
            from: Timestamp::from_unix_millis(-3),
            to: Timestamp::from_unix_millis(1_700_000_000_000),
            lost_entries: 42,
        }
    }

    #[test]
    fn state_round_trips_with_and_without_a_gap() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        let dir = volume.dir();
        assert_eq!(load(dir).unwrap(), Loaded::Missing);
        for state in [
            State {
                reserved_seq: 4097,
                floor_seq: 12,
                gap: None,
            },
            State {
                reserved_seq: u64::MAX,
                floor_seq: u64::MAX,
                gap: Some(gap()),
            },
        ] {
            store(dir, &state).unwrap();
            assert_eq!(load(dir).unwrap(), Loaded::Valid(state));
        }
        // No temporary file is left behind.
        assert!(dir.metadata(STATE_TMP).is_err());
    }

    #[test]
    fn a_damaged_or_foreign_file_is_invalid_not_trusted() {
        let tmp = TempDir::new().unwrap();
        let volume = SpoolVolume::open(tmp.path()).unwrap();
        let dir = volume.dir();
        let good = encode(&State {
            reserved_seq: 10,
            floor_seq: 3,
            gap: Some(gap()),
        });
        for bad in [
            Vec::new(),
            b"garbage".to_vec(),
            good[..LEN - 1].to_vec(),
            [good.as_slice(), b"x"].concat(),
            {
                let mut b = good.to_vec();
                b[20] ^= 1;
                b
            },
            {
                // A good checksum over a gap that runs backwards is still refused.
                let mut b = good.to_vec();
                b[32..40].copy_from_slice(&i64::MAX.to_le_bytes());
                let crc = crc32fast::hash(&b[..56]);
                b[56..60].copy_from_slice(&crc.to_le_bytes());
                b
            },
        ] {
            dir.write(STATE_FILE, &bad).unwrap();
            assert_eq!(load(dir).unwrap(), Loaded::Invalid, "{bad:?}");
        }
    }
}
