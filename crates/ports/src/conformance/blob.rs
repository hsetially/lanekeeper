use bytes::Bytes;

use crate::{BlobError, BlobStore, MAX_BLOB_BYTES, MAX_GET_MANY, content_hash};

/// - The address is the SHA-256 of the exact bytes (CRLF is preserved), and storing is idempotent.
/// - Unknown hashes are `None` (not an error); `get_many` skips unknown hashes, keeps first-request order
///   and returns each blob once.
/// - Size limits: a blob over [`MAX_BLOB_BYTES`] is `TooLarge`; more than [`MAX_GET_MANY`] hashes is `TooMany`.
pub async fn blob_store<B: BlobStore + ?Sized>(b: &B) {
    let empty = b.put(Bytes::new()).await.expect("empty blob");
    assert_eq!(empty, content_hash(b""), "address of the empty blob");
    assert_eq!(b.get(&empty).await.unwrap(), Some(Bytes::new()));

    let crlf = Bytes::from_static(b"a: 1\r\nb: 2\r\n");
    let h1 = b.put(crlf.clone()).await.unwrap();
    assert_eq!(h1, content_hash(&crlf));
    assert_eq!(b.put(crlf.clone()).await.unwrap(), h1, "put is idempotent");
    assert_eq!(
        b.get(&h1).await.unwrap(),
        Some(crlf),
        "bytes come back exactly as stored"
    );

    let h2 = b.put(Bytes::from_static(b"other")).await.unwrap();
    let unknown = content_hash(b"never stored");
    assert_eq!(b.get(&unknown).await.unwrap(), None, "unknown hash");

    let many = b.get_many(&[h2, unknown, h1, h2]).await.unwrap();
    let hashes: Vec<_> = many.iter().map(|(h, _)| *h).collect();
    assert_eq!(
        hashes,
        vec![h2, h1],
        "first-request order, unknown skipped, no repeats"
    );
    for (h, bytes) in &many {
        assert_eq!(*h, content_hash(bytes), "each returned blob matches its address");
    }

    let big = Bytes::from(vec![7u8; MAX_BLOB_BYTES + 1]);
    assert_eq!(b.put(big).await, Err(BlobError::TooLarge));
    let at_limit = Bytes::from(vec![8u8; MAX_BLOB_BYTES]);
    assert!(
        b.put(at_limit).await.is_ok(),
        "a blob of exactly the maximum size is accepted"
    );

    let too_many = vec![unknown; MAX_GET_MANY + 1];
    assert_eq!(b.get_many(&too_many).await, Err(BlobError::TooMany));
    assert!(b.get_many(&[]).await.unwrap().is_empty());
}
