//! P7, a file's content through the API, cold (`P7.file_content_cold`, `hub-registry/file_content_cold`, a harness, p95, 80 ms):
//! `GET .../files/content` for a file whose blob is not in the blob cache, so that it is read from the `blobs` table, over the read
//! bed, as the router answers it.
//!
//! Every sample is a different file with a different blob (about 400 of them, 24 from each swimlane at even intervals of its YAML
//! files, duplicates dropped, since swimlanes share files), read once from a hub whose caches are empty. The files are the ordinary ones of the fixtures, large and
//! small as they come; nothing is picked for being quick.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use std::collections::BTreeSet;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const PER_LANE: usize = 24;

fn enc(path: &str) -> String {
    path.replace('/', "%2F")
}

#[tokio::main]
async fn main() {
    let Some(mut bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/file_content_cold: SKIPPED (no database)");
        return;
    };
    let mut seen = BTreeSet::new();
    let mut uris: Vec<(String, usize)> = Vec::new();
    for (i, lane) in bed.bed.lanes.iter().enumerate() {
        let editable = bed.bed.editable(i);
        let step = (editable.len() / PER_LANE).max(1);
        for f in editable.iter().step_by(step).take(PER_LANE) {
            let (path, bytes) = &lane.files[*f];
            if seen.insert(ports::content_hash(bytes)) {
                uris.push((
                    format!(
                        "/api/v1/swimlanes/{}/files/content?path={}",
                        lane.id,
                        enc(path.as_str())
                    ),
                    bytes.len(),
                ));
            }
        }
    }
    assert!(uris.len() >= 300, "{} distinct files", uris.len());
    // A hub that has just started: nothing cached by the service or the blob store (the database is as warm as it was).
    bed.restart();
    let mut samples = Vec::new();
    for (uri, size) in &uris {
        let before = bed.blobs.cache().len();
        let r = bed.ok(uri).await;
        assert_eq!(
            bed.blobs.cache().len(),
            before + 1,
            "{uri}: the blob was cached already (or not cached by the read), which is not a cold read"
        );
        assert!(
            r.bytes >= *size,
            "the answer holds the file ({} of {size} bytes)",
            r.bytes
        );
        samples.push(r.ms);
    }
    eprintln!(
        "hub-registry/file_content_cold: {} over {} distinct blobs",
        summary(&samples),
        uris.len()
    );
    let path = write_result("hub-registry/file_content_cold", "ms", &samples);
    eprintln!("hub-registry/file_content_cold: {}", path.display());
}
