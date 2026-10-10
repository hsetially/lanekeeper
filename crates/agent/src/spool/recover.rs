//! Reading a spool back (D74): which segments exist, what each holds, and what to do about the ones that end in damage.
//!
//! Every segment is read once, in id order, and every checksum is checked ([`segment::scan`]); a record is then handed to
//! the index in file order, which applies the same rules as a live append (a delta is complete or it is not). What this
//! does not do is change anything on disk: it reports which damaged segments to cut or remove, and the caller does that
//! after the loss has been written down.

use std::collections::BTreeMap;
use std::io;

use cap_std::fs::Dir;
use tracing::warn;

use super::Recovery;
use super::index::{Index, Limits};
use super::segment::{self, Damage, SEGMENT_HEADER_LEN};
use crate::ops::Metrics;

/// What reading the spool found.
pub struct Recovered {
    pub index: Index,
    pub recovery: Recovery,
    /// The highest sequence number in any segment, settled or not.
    pub max_seq: u64,
    /// Damaged segments: `Some(len)` to cut the file to `len` bytes, `None` to remove it.
    pub repairs: BTreeMap<u64, Option<u64>>,
}

/// The ids of the segment files, oldest first. A half-written state file from a crash is removed; anything else that is
/// not a segment is left alone.
pub fn list_segments(dir: &Dir) -> io::Result<Vec<u64>> {
    let mut ids = Vec::new();
    for entry in dir.entries()? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == super::state::STATE_TMP {
            let _ = dir.remove_file(name);
        } else if let Some(id) = segment::parse_name(name) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// Read the segments `ids`. Records below `floor` were settled by an earlier run and are skipped.
pub fn read_all(
    dir: &Dir,
    ids: &[u64],
    limits: Limits,
    floor: u64,
    now: i64,
    metrics: &Metrics,
) -> Recovered {
    let mut found = Recovered {
        index: Index::new(limits),
        recovery: Recovery::default(),
        max_seq: 0,
        repairs: BTreeMap::new(),
    };
    for &id in ids {
        read_one(dir, id, floor, now, metrics, &mut found);
    }
    found
}

fn read_one(dir: &Dir, id: u64, floor: u64, now: i64, metrics: &Metrics, found: &mut Recovered) {
    let scan = match dir.open(segment::name(id)).and_then(segment::scan) {
        Ok(scan) => scan,
        Err(e) => {
            // A segment that cannot be read at all (a bad block, say) is skipped and kept, so that it can be looked
            // at; what it held is reported as lost.
            warn!(segment = id, kind = ?e.kind(), "a spool segment cannot be read");
            found.recovery.damaged_segments += 1;
            found.index.note_loss(now, now, 1);
            return;
        }
    };
    if scan.file_len == 0 {
        let _ = dir.remove_file(segment::name(id));
        return;
    }
    found.recovery.segments += 1;
    if matches!(scan.damage, Some(Damage::BadMagic)) {
        found.repairs.insert(id, None);
    } else {
        found.index.register_segment(id, scan.valid_len);
        if scan.damage.is_some() {
            found.repairs.insert(id, Some(scan.valid_len));
        }
    }
    let mut last_seen = None;
    let mut broken = scan.damage.is_some();
    for record in &scan.found {
        found.max_seq = found.max_seq.max(record.meta.seq);
        if record.meta.seq < floor {
            // Settled in an earlier run: acknowledged, or dropped and already reported.
            found.index.skip(record.meta.seq);
            continue;
        }
        if found
            .index
            .observe(&record.meta, id, record.offset, record.frame_len)
            .is_ok()
        {
            found.recovery.records += 1;
            last_seen = Some(record.meta.last_ms);
        } else {
            // A sequence number that goes backwards: whatever follows is not this spool's own writing.
            broken = true;
            found.repairs.insert(id, Some(record.offset));
            break;
        }
    }
    if broken {
        found.recovery.damaged_segments += 1;
        metrics.spool_damaged();
        found.index.note_loss(last_seen.unwrap_or(now), now, 1);
        warn!(
            segment = id,
            readable = scan.valid_len,
            file = scan.file_len,
            "a spool segment ends in damage; what follows the last whole record is lost"
        );
    }
}

/// Make a damaged segment safe to leave on disk: cut off what follows its last whole record, or remove it when nothing
/// whole is left.
pub fn repair(dir: &Dir, id: u64, keep: Option<u64>) {
    let name = segment::name(id);
    match keep {
        Some(len) if len > SEGMENT_HEADER_LEN => {
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true);
            let cut = dir
                .open_with(&name, &options)
                .and_then(|file| file.set_len(len).and_then(|()| file.sync_all()));
            match cut {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => warn!(segment = id, kind = ?e.kind(), "a damaged spool segment could not be cut"),
            }
        }
        _ => {
            let _ = dir.remove_file(&name);
        }
    }
}
