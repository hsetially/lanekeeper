//! A directory that looks like one swimlane of the target scale, for tests that must not depend on generated fixtures:
//! about `n` files in a few hundred directories, a mix of small property files and a few large YAML files.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

/// Write `n` files below `root`. Deterministic.
pub fn populate(root: &Path, n: usize) {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in 0..n {
        let mut dir = root.join(format!("svc-{:03}", i % 150));
        if i % 5 == 0 {
            dir.push("resources");
        }
        fs::create_dir_all(&dir).unwrap();
        let size = match i % 40 {
            0 => 200_000,
            1..=4 => 20_000,
            _ => 1_500,
        };
        let mut body = Vec::with_capacity(size);
        while body.len() < size {
            body.extend_from_slice(format!("key{}: value{}\r\n", i, next() % 100_000).as_bytes());
        }
        body.truncate(size);
        fs::write(dir.join(format!("file-{i:05}.yml")), body).unwrap();
    }
}
