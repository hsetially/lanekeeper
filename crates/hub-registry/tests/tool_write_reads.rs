//! A registration is not an observation, and no reader of history can mistake one for the other (T1, A10).
//!
//! The first seam kept a registration as a row of `file_observations`, and `is_version_of` (which decides whether
//! `ContentSource::Blob { hash }` may serve the blob of `hash` under a path: "a version this path has had") answered true for any hash in
//! that table. The content that someone was about to write, often a draft's or a proposal's blob that the blob store holds already, was
//! therefore readable by any viewer of the swimlane before the write happened, or if it never happened. The registrations are a table of
//! their own now.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use bytes::Bytes;
use domain::{ContentSource, NfsPath, SwimlaneId};
use ports::{BlobStore, ReadError, RegistryRead, content_hash};
use support::reads::Reads;

#[tokio::test]
async fn a_registered_hash_is_not_a_version_of_its_path_and_its_blob_is_not_served() {
    let Some(mut r) = Reads::new(&["sit1"]).await else {
        return;
    };
    r.world.inventory(&[("a/one.yml", "x: 1\n")]).await;
    let (viewer, lane, one) = (
        Reads::viewer(),
        SwimlaneId::parse("sit1").unwrap(),
        NfsPath::parse("a/one.yml").unwrap(),
    );
    // A draft's blob: in the store, never the content of the file.
    let draft = Bytes::from_static(b"x: 99\n");
    let hash = r.world.rig.blobs.put(draft.clone()).await.unwrap();
    assert_eq!(hash, content_hash(b"x: 99\n"));
    let served = |r: &Reads| {
        let (service, viewer, lane, one) = (r.service.clone(), viewer.clone(), lane.clone(), one.clone());
        async move {
            service
                .content(&viewer, &lane, &one, ContentSource::Blob { hash })
                .await
        }
    };
    assert_eq!(
        served(&r).await.unwrap_err(),
        ReadError::NotFound,
        "never registered, never a version"
    );

    // The write path registers it (a committed registration) and has not written the file.
    r.world.rig.registered("a/one.yml", "x: 99\n", "req-1", 5).await;
    assert_eq!(
        served(&r).await.unwrap_err(),
        ReadError::NotFound,
        "a registration is an expectation, not a version: the blob is not readable under the path"
    );
    r.world
        .rig
        .registered_delete("a/one.yml", "x: 1\n", "req-2", 5)
        .await;
    assert_eq!(
        r.world
            .rig
            .count("SELECT count(*) FROM file_observations WHERE path = 'a/one.yml'")
            .await,
        1,
        "and neither kind of registration wrote an observation: the history is the first inventory"
    );

    // Once the agent has seen the file with that content it is a version of the path, and is served.
    r.world.nfs_change(&[("a/one.yml", "x: 99\n")]).await;
    let content = served(&r).await.unwrap();
    assert_eq!(content.bytes.as_deref(), Some(&b"x: 99\n"[..]));
}
