//! S10: what the write path logs holds paths, hashes and ids, never file content, a secret, or a value of the request.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use domain::{Role, SwimlaneId};
use ports::WriteService;
use support::fake_agent::Fault;
use support::log_capture;
use support::rig::{Rig, at, ctx, edit};

const MARKER: &str = "content-marker-7f3a91";

#[tokio::test]
async fn write_path_logs_hold_paths_hashes_and_ids_only() {
    let (capture, _guard) = log_capture::install();
    let rig = Rig::new(db!()).await;
    let s: SwimlaneId = rig.swimlane("sit1", true).await;
    rig.seed(&s, &[("a.yml", "x: 1\n"), ("b.yml", "k: v\n")]);
    let editor = rig.user(1, Role::Editor).await;
    let careful = rig.approval_user(2, Role::Editor).await;
    let key = ["AKIA", "IOSFODNN7EXAMPLE"].concat();

    // An applied write, a conflict, a secret, a parse error, a proposal, a lost reply, a replay: each leaves a log line or none.
    let hub = &rig.hub;
    hub.edit(
        &ctx(&editor, "k-1"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-2"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}-2\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-3"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: {key}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&editor, "k-4"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: [{MARKER}\n")),
    )
    .await
    .unwrap();
    hub.edit(
        &ctx(&careful, "k-5"),
        edit(&s, "b.yml", at("k: v\n"), &format!("k: {MARKER}-3\n")),
    )
    .await
    .unwrap();
    rig.agent.fault_next_write(Fault::LandThenTimeout);
    let _ = hub
        .edit(
            &ctx(&editor, "k-6"),
            edit(&s, "b.yml", at("k: v\n"), &format!("k: {MARKER}-4\n")),
        )
        .await;
    rig.audit.fail_next();
    let _ = hub
        .edit(
            &ctx(&editor, "k-7"),
            edit(&s, "c.yml", domain::Expected::Absent, &format!("c: {MARKER}-5\n")),
        )
        .await;
    hub.edit(
        &ctx(&editor, "k-1"),
        edit(&s, "a.yml", at("x: 1\n"), &format!("mark: {MARKER}\n")),
    )
    .await
    .unwrap();

    let logged = capture.text();
    assert!(!logged.contains(MARKER), "file content in the log:\n{logged}");
    assert!(!logged.contains(&key), "a secret in the log");
    assert!(!logged.contains("user1@example.com"), "an email in the log");
    assert!(!logged.contains("Bearer"), "credentials in the log");
}
