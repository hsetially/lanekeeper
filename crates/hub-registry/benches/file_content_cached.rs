//! P7, a file's content through the API, cached (`P7.file_content_cached`, `hub-registry/file_content_cached`, a harness, p95, 30 ms):
//! `GET .../files/content` for files whose blobs the hub's blob cache holds, over the read bed (the real `PgBlobStore` in front of the
//! `blobs` table), as the router answers it: the guard, the file's row, the blob from the cache, the decoding to text, the JSON.
//!
//! Each of the 40 swimlanes contributes its median-size and its largest editable YAML file (up to 200 kB, which at the fixtures is the
//! 6,000-line file), so the tail is the large files. Every file is read once first to fill the cache (not a sample), and the samples
//! are the next four rounds over all of them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

mod common;

use common::reads::{ReadBed, summary};
use common::support::perf::write_result;
use hub_registry::RegistryConfig;

const ROUNDS: usize = 4;

fn enc(path: &str) -> String {
    path.replace('/', "%2F")
}

#[tokio::main]
async fn main() {
    let Some(bed) = Box::pin(ReadBed::new(40, RegistryConfig::default())).await else {
        eprintln!("hub-registry/file_content_cached: SKIPPED (no database)");
        return;
    };
    let mut uris: Vec<(String, usize)> = Vec::new();
    for (i, lane) in bed.bed.lanes.iter().enumerate() {
        let mut editable = bed.bed.editable(i);
        editable.sort_by_key(|&f| (lane.files[f].1.len(), f));
        for f in [editable[editable.len() / 2], *editable.last().unwrap()] {
            let (path, bytes) = &lane.files[f];
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
    // Fill the cache, and check that what comes back is the file.
    for (uri, size) in &uris {
        let answer = bed.json(uri).await;
        assert_eq!(answer["content_withheld"], false, "{uri}");
        assert_eq!(answer["size"].as_u64().unwrap(), *size as u64, "{uri}");
        assert!(answer["text"].as_str().is_some_and(|t| !t.is_empty()), "{uri}");
    }
    let largest = uris.iter().map(|(_, s)| *s).max().unwrap();
    let mut samples = Vec::new();
    for _ in 0..ROUNDS {
        for (uri, _) in &uris {
            samples.push(bed.ok(uri).await.ms);
        }
    }
    eprintln!(
        "hub-registry/file_content_cached: {} over {} files, the largest {largest} bytes",
        summary(&samples),
        uris.len()
    );
    let path = write_result("hub-registry/file_content_cached", "ms", &samples);
    eprintln!("hub-registry/file_content_cached: {}", path.display());
}
