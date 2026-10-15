//! T2, byte faithfulness (rule 11, D17): a write keeps the file's own line endings, encoding and BOM; only `change_eol` changes
//! the style; a new file takes the dominant style of its folder; binary content goes through untouched. Real Postgres through
//! `LK_TEST_PG_ADMIN_URL` or a container.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use bytes::Bytes;
use domain::{ChangeEol, EditRequest, Expected, RejectReason, Role, SwimlaneId, WriteOutcome};
use ports::conformance::sample::nfs;
use ports::{BlobStore, content_hash};
use support::rig::{Rig, applied, ctx};

fn utf16le(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn at_bytes(bytes: &[u8]) -> Expected {
    Expected::Hash {
        hash: content_hash(bytes),
    }
}

fn request(s: &SwimlaneId, path: &str, expected: Expected, content: &[u8]) -> EditRequest {
    EditRequest {
        swimlane: s.clone(),
        path: nfs(path),
        expected,
        content: Bytes::copy_from_slice(content),
        change_eol: None,
    }
}

/// A rig with a live registry (it lists folders with the real format of each file) and an unlocked swimlane.
async fn rig() -> Option<(Rig, SwimlaneId)> {
    let rig = Rig::with_live_registry(support::pg::TestDb::ready().await?, 4).await;
    let s = rig.swimlane("sit1", true).await;
    Some((rig, s))
}

async fn edit_as_editor(rig: &Rig, key: &str, r: EditRequest) -> WriteOutcome {
    use ports::WriteService;
    let editor = rig.user(1, Role::Editor).await;
    rig.hub.edit(&ctx(&editor, key), r).await.unwrap()
}

#[tokio::test]
async fn crlf_file_edited_from_lf_input_keeps_crlf() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"a: 1\r\nb: 2\r\nc: 3\r\n";
    rig.agent.put(&s, &nfs("a.yml"), old);

    let out = edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.yml", at_bytes(old), b"a: 1\nb: 9\nc: 3\n"),
    )
    .await;

    let written = b"a: 1\r\nb: 9\r\nc: 3\r\n";
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&written[..]));
    // The hash the outcome names is the hash of the bytes that were written, not of the input.
    assert_eq!(applied(&out).0, Some(content_hash(written)));
}

#[tokio::test]
async fn stored_diff_contains_only_changed_lines() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"a: 1\r\nb: 2\r\nc: 3\r\n";
    rig.agent.put(&s, &nfs("a.yml"), old);

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.yml", at_bytes(old), b"a: 1\nb: 9\nc: 3\n"),
    )
    .await;

    let chain = rig.chain().await;
    let diff = chain[0].event.diff.clone().unwrap();
    assert!(diff.contains("-b: 2") && diff.contains("+b: 9"), "{diff}");
    // The untouched lines are context only, and the whole change is one hunk of one removed and one added line.
    let changed: Vec<&str> = diff
        .lines()
        .filter(|l| {
            (l.starts_with('-') || l.starts_with('+')) && !l.starts_with("---") && !l.starts_with("+++")
        })
        .collect();
    assert_eq!(changed, vec!["-b: 2", "+b: 9"], "{diff}");
    assert!(
        !diff.contains("line endings only") && !diff.contains('\r'),
        "{diff}"
    );
    // The version kept for the event is the one that went to NFS.
    let kept = rig
        .blobs
        .get(&content_hash(b"a: 1\r\nb: 9\r\nc: 3\r\n"))
        .await
        .unwrap();
    assert_eq!(kept.as_deref(), Some(&b"a: 1\r\nb: 9\r\nc: 3\r\n"[..]));
}

#[tokio::test]
async fn bom_is_preserved() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"\xEF\xBB\xBFx: 1\r\n";
    rig.agent.put(&s, &nfs("a.yml"), old);

    edit_as_editor(&rig, "k-1", request(&s, "a.yml", at_bytes(old), b"x: 2\n")).await;

    assert_eq!(
        rig.nfs_of(&s, "a.yml").as_deref(),
        Some(&b"\xEF\xBB\xBFx: 2\r\n"[..])
    );
}

#[tokio::test]
async fn a_bom_the_file_does_not_have_is_not_added() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"x: 1\n";
    rig.agent.put(&s, &nfs("a.yml"), old);

    // An editor that put a BOM on its text does not give the file one.
    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.yml", at_bytes(old), b"\xEF\xBB\xBFx: 2\n"),
    )
    .await;

    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"x: 2\n"[..]));
}

#[tokio::test]
async fn a_utf16_file_stays_utf16_with_its_mark_and_line_endings() {
    let Some((rig, s)) = rig().await else { return };
    let old = utf16le("a=1\r\nb=2\r\n");
    rig.agent.put(&s, &nfs("a.properties"), &old);

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.properties", at_bytes(&old), b"a=1\nb=3\n"),
    )
    .await;

    assert_eq!(
        rig.nfs_of(&s, "a.properties").as_deref(),
        Some(&utf16le("a=1\r\nb=3\r\n")[..])
    );
}

#[tokio::test]
async fn a_latin1_file_stays_latin1() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"name=Caf\xE9\n";
    rig.agent.put(&s, &nfs("a.properties"), old);

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.properties", at_bytes(old), "name=Caf\u{e9}s\n".as_bytes()),
    )
    .await;

    assert_eq!(
        rig.nfs_of(&s, "a.properties").as_deref(),
        Some(&b"name=Caf\xE9s\n"[..])
    );
}

#[tokio::test]
async fn text_a_latin1_file_cannot_hold_is_refused_and_writes_nothing() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"name=Caf\xE9\n";
    rig.agent.put(&s, &nfs("a.properties"), old);

    let out = edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.properties", at_bytes(old), "name=\u{20ac}\n".as_bytes()),
    )
    .await;

    assert!(
        matches!(
            out,
            WriteOutcome::Rejected {
                reason: RejectReason::InvalidContent,
                ..
            }
        ),
        "{out:?}"
    );
    assert_eq!(rig.nfs_of(&s, "a.properties").as_deref(), Some(&old[..]));
    assert_eq!(rig.count("audit_events").await, 0);
}

#[tokio::test]
async fn change_eol_only_when_explicit() {
    let Some((rig, s)) = rig().await else { return };
    let crlf = b"a: 1\r\nb: 2\r\n";
    rig.agent.put(&s, &nfs("a.yml"), crlf);
    let lf = b"c: 1\nd: 2\n";
    rig.agent.put(&s, &nfs("c.yml"), lf);

    // Asked: LF file to CRLF, CRLF file to LF.
    let mut to_crlf = request(&s, "c.yml", at_bytes(lf), b"c: 1\nd: 3\n");
    to_crlf.change_eol = Some(ChangeEol::Crlf);
    edit_as_editor(&rig, "k-1", to_crlf).await;
    assert_eq!(rig.nfs_of(&s, "c.yml").as_deref(), Some(&b"c: 1\r\nd: 3\r\n"[..]));

    let mut to_lf = request(&s, "a.yml", at_bytes(crlf), b"a: 1\nb: 3\n");
    to_lf.change_eol = Some(ChangeEol::Lf);
    edit_as_editor(&rig, "k-2", to_lf).await;
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&b"a: 1\nb: 3\n"[..]));
}

#[tokio::test]
async fn without_change_eol_a_crlf_file_is_never_converted_even_when_the_input_is_crlf_free() {
    let Some((rig, s)) = rig().await else { return };
    let crlf = b"a: 1\r\nb: 2\r\n";
    rig.agent.put(&s, &nfs("a.yml"), crlf);

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "a.yml", at_bytes(crlf), b"a: 1\nb: 2\nc: 3\n"),
    )
    .await;

    assert_eq!(
        rig.nfs_of(&s, "a.yml").as_deref(),
        Some(&b"a: 1\r\nb: 2\r\nc: 3\r\n"[..])
    );
}

#[tokio::test]
async fn new_file_takes_the_dominant_style_of_its_folder() {
    let Some((rig, s)) = rig().await else { return };
    rig.agent.put(&s, &nfs("svc/one.yml"), b"a: 1\r\n");
    rig.agent.put(&s, &nfs("svc/two.yml"), b"b: 1\r\n");
    rig.agent.put(&s, &nfs("svc/three.yml"), b"c: 1\n");
    rig.agent.put(&s, &nfs("lf/one.yml"), b"a: 1\n");

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "svc/new.yml", Expected::Absent, b"n: 1\nm: 2\n"),
    )
    .await;
    assert_eq!(
        rig.nfs_of(&s, "svc/new.yml").as_deref(),
        Some(&b"n: 1\r\nm: 2\r\n"[..])
    );

    edit_as_editor(
        &rig,
        "k-2",
        request(&s, "lf/new.yml", Expected::Absent, b"n: 1\r\nm: 2\r\n"),
    )
    .await;
    assert_eq!(
        rig.nfs_of(&s, "lf/new.yml").as_deref(),
        Some(&b"n: 1\nm: 2\n"[..])
    );
}

#[tokio::test]
async fn a_new_file_in_an_empty_or_tied_folder_keeps_the_style_it_came_with() {
    let Some((rig, s)) = rig().await else { return };
    rig.agent.put(&s, &nfs("tie/one.yml"), b"a: 1\r\n");
    rig.agent.put(&s, &nfs("tie/two.yml"), b"b: 1\n");

    edit_as_editor(
        &rig,
        "k-1",
        request(&s, "tie/new.yml", Expected::Absent, b"n: 1\r\n"),
    )
    .await;
    assert_eq!(rig.nfs_of(&s, "tie/new.yml").as_deref(), Some(&b"n: 1\r\n"[..]));

    edit_as_editor(
        &rig,
        "k-2",
        request(&s, "empty/new.yml", Expected::Absent, b"n: 1\r\nm: 2\n"),
    )
    .await;
    // Nothing to follow: the content is written as it came.
    assert_eq!(
        rig.nfs_of(&s, "empty/new.yml").as_deref(),
        Some(&b"n: 1\r\nm: 2\n"[..])
    );
}

#[tokio::test]
async fn binary_content_is_written_untouched() {
    let Some((rig, s)) = rig().await else { return };
    let old = b"BM\x00\x01\r\n\x02";
    rig.agent.put(&s, &nfs("logo.bmp"), old);
    let new = b"BM\x00\x09\r\n\x0A\x0D\x0A\x00\xEF\xBB\xBF";

    edit_as_editor(&rig, "k-1", request(&s, "logo.bmp", at_bytes(old), new)).await;

    assert_eq!(rig.nfs_of(&s, "logo.bmp").as_deref(), Some(&new[..]));
}

#[tokio::test]
async fn content_that_grows_past_the_limit_when_converted_is_refused() {
    let Some((rig, s)) = rig().await else { return };
    // Under 2 MiB as LF (15 bytes a line), over it once each line has a CR as the file's CRLF style asks.
    let old = b"a: 1\r\n";
    rig.agent.put(&s, &nfs("big.txt"), old);
    let content = "k: 01234567890\n".repeat(135_000);
    assert!(content.len() < 2 * 1024 * 1024 && content.len() + 135_000 > 2 * 1024 * 1024);

    let out = edit_as_editor(
        &rig,
        "k-1",
        request(&s, "big.txt", at_bytes(old), content.as_bytes()),
    )
    .await;

    assert!(
        matches!(
            out,
            WriteOutcome::Rejected {
                reason: RejectReason::TooLarge,
                ..
            }
        ),
        "{out:?}"
    );
    assert_eq!(rig.nfs_of(&s, "big.txt").as_deref(), Some(&old[..]));
    assert_eq!(rig.count("audit_events").await, 0);
}

#[tokio::test]
async fn a_proposal_holds_the_content_in_the_form_of_the_file() {
    use ports::WriteService;
    let Some((rig, s)) = rig().await else { return };
    let old = b"a: 1\r\nb: 2\r\n";
    rig.agent.put(&s, &nfs("a.yml"), old);
    let careful = rig.approval_user(6, Role::Editor).await;

    let out = rig
        .hub
        .edit(
            &ctx(&careful, "p-1"),
            request(&s, "a.yml", at_bytes(old), b"a: 1\nb: 9\n"),
        )
        .await
        .unwrap();

    let WriteOutcome::ProposalCreated { id } = out else {
        panic!("expected a proposal: {out:?}");
    };
    // Approving it later writes these bytes, so the hash and the stored version are those of the CRLF file.
    let new_hash: Vec<u8> = sqlx::query_scalar("SELECT new_hash FROM proposals WHERE id = $1")
        .bind(id.get())
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(new_hash, content_hash(b"a: 1\r\nb: 9\r\n").as_bytes().to_vec());
    let kept = rig.blobs.get(&content_hash(b"a: 1\r\nb: 9\r\n")).await.unwrap();
    assert_eq!(kept.as_deref(), Some(&b"a: 1\r\nb: 9\r\n"[..]));
    assert_eq!(rig.nfs_of(&s, "a.yml").as_deref(), Some(&old[..]));
}
