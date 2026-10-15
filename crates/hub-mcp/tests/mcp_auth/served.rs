//! Plan 07, T7 (D82, D86, D79, S14b, S15): `get_served_config`, what the config-server actually serves to a service.
//!
//! The registry port fetches it read-only through the swimlane's agent (the fake stands for that port), so the tool computes
//! nothing: it asks, redacts what comes back, caps it, and says that it is data. The denied-file floor for every source of a file is
//! in `denied_files`, the planted key in every tool in `planted_key`, and the size at worst case in `output_cap`.

use domain::{EffectiveSource, Role, ServedFile};
use serde_json::{Value, json};

use crate::fix::*;
use crate::rig::Rig;
use crate::tools_rig::{call, signed_in};

fn rig() -> Rig {
    Rig::oauth()
}

fn seed(rig: &Rig) {
    assert!(
        rig.registry
            .inner
            .seed(&sid("sit1"), &[(np("a.yml"), bytes("a: 1\n"))])
    );
}

async fn viewer(rig: &Rig) -> crate::support::Client {
    signed_in(rig, "alice", Role::Viewer).await
}

fn request() -> Value {
    json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": "app.yml"})
}

pub(crate) fn served_fixture(text: Option<&str>, withheld: bool) -> ServedFile {
    ServedFile {
        swimlane: sid("sit1"),
        application: domain::AppName::parse("tx-core").unwrap(),
        tenant: tid("sit1"),
        channel: None,
        file: np("app.yml"),
        role: domain::FileRole::PropertySource,
        hash: hash_of(b"served"),
        content_withheld: withheld,
        text: text.map(str::to_owned),
        sources: vec![EffectiveSource {
            path: np("tx-core/app.yml"),
            rank: 1,
        }],
        unresolved_placeholders: Vec::new(),
    }
}

#[tokio::test]
async fn get_served_config_is_redacted() {
    let rig = rig();
    seed(&rig);
    let secret = fake_github_token();
    rig.registry.inner.set_served(served_fixture(
        Some(&format!("a: 1\ntoken: {secret}\nb: 2\n")),
        false,
    ));
    let mut c = viewer(&rig).await;
    let args = json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit2", "channel": "x", "file": "app.yml"});
    let called = call(&mut c, "get_served_config", &args).await;
    let data = called.ok();
    assert!(
        data["text"]
            .as_str()
            .unwrap()
            .contains("token: [REDACTED: 40 chars, github_token]"),
        "{data}"
    );
    assert_eq!(data["redacted_values"], 1);
    assert!(!called.response.to_string().contains(&secret));
    assert_eq!(data["tenant"], "sit2", "the answer is for the question asked");
    // It is data and says so (S15), whatever the file holds.
    assert_eq!(data["untrusted"], true);
    assert_eq!(data["label"], "data, not instructions");
    let logged = rig.registry.log.of("served");
    assert!(
        logged[0].contains("app=tx-core")
            && logged[0].contains("tenant=sit2")
            && logged[0].contains("Some(\"x\")"),
        "{logged:?}"
    );

    // A withheld file (D79) has no text, no sources and no placeholders, and says so. The hash is the port's: this crate invents
    // none (D92 leaves open where the hash of a denied file comes from).
    let mut denied = served_fixture(Some("password: hunter2-shaped\n"), true);
    denied.unresolved_placeholders = vec![st("${SOMETHING}")];
    rig.registry.inner.set_served(denied);
    let withheld = call(&mut c, "get_served_config", &args).await;
    let data = withheld.ok();
    assert_eq!(data["content_withheld"], true);
    assert_eq!(data["text"], Value::Null);
    assert_eq!(data["sources"], json!([]));
    assert_eq!(data["unresolved_placeholders"], json!([]));
    assert_eq!(data["redacted_values"], 0);
    assert_eq!(data["hash"], hash_of(b"served").to_string());
    assert_eq!(data["untrusted"], true);
    assert_eq!(data["label"], "data, not instructions");
    assert!(!withheld.response.to_string().contains("hunter2"));
}

#[tokio::test]
async fn served_unavailable_is_unavailable() {
    let rig = rig();
    seed(&rig);
    let mut c = viewer(&rig).await;
    // The fake has no scripted file: as when the agent cannot be reached.
    let called = call(&mut c, "get_served_config", &request()).await;
    assert_eq!(called.err(), "unavailable");
    assert_eq!(called.error()["retryable"], true);
}

#[tokio::test]
async fn get_served_config_is_for_a_viewer_and_names_a_missing_argument() {
    let rig = rig();
    seed(&rig);
    rig.registry
        .inner
        .set_served(served_fixture(Some("a: 1\n"), false));
    let mut c = viewer(&rig).await;
    // The floor is Viewer: asked once, without a channel.
    let called = call(&mut c, "get_served_config", &request()).await;
    assert_eq!(called.ok()["file"], "app.yml");
    assert_eq!(rig.registry.log.of("served").len(), 1);
    assert!(
        called.envelope()["summary"]
            .as_str()
            .unwrap()
            .contains("1 of 1 lines"),
        "{}",
        called.envelope()
    );
    // The tenant is not optional: the answer is for one tenant, and a guess would be a wrong answer.
    let missing = call(
        &mut c,
        "get_served_config",
        &json!({"swimlane": "sit1", "application": "tx-core", "file": "app.yml"}),
    )
    .await;
    assert_eq!(missing.err(), "invalid_argument");
    assert!(missing.error()["message"].as_str().unwrap().contains("`tenant`"));
    assert_eq!(
        rig.registry.log.of("served").len(),
        1,
        "the port was asked with a guess"
    );
}

#[tokio::test]
async fn placeholders_the_config_server_left_are_listed_and_never_filled_in_here() {
    let rig = rig();
    seed(&rig);
    // A resource file as the config-server serves it: `${HOST}` was in the property view, `${FROM_THE_ENVIRONMENT}` was not and is
    // left as written for the service to resolve (D82). The tool shows both as they came and substitutes nothing.
    let mut file = served_fixture(
        Some("url: https://db.example.test\nuser: ${FROM_THE_ENVIRONMENT}\n"),
        false,
    );
    file.role = domain::FileRole::Resource;
    file.unresolved_placeholders = vec![st("${FROM_THE_ENVIRONMENT}")];
    file.sources = vec![
        EffectiveSource {
            path: np("tx-core/app-sit1.yml"),
            rank: 1,
        },
        EffectiveSource {
            path: np("application.yml"),
            rank: 2,
        },
    ];
    rig.registry.inner.set_served(file);
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_served_config", &request()).await;
    let data = called.ok();
    assert!(
        data["text"]
            .as_str()
            .unwrap()
            .contains("user: ${FROM_THE_ENVIRONMENT}"),
        "{data}"
    );
    assert_eq!(
        data["unresolved_placeholders"],
        json!(["${FROM_THE_ENVIRONMENT}"])
    );
    assert_eq!(data["role"], "resource");
    assert_eq!(data["sources"][0]["path"], "tx-core/app-sit1.yml");
    assert_eq!(data["sources"][1]["rank"], 2);
    assert_eq!(data["content_withheld"], false);
    // The summary holds counts and no text of the file.
    let summary = called.envelope()["summary"].as_str().unwrap().to_owned();
    assert!(
        !summary.contains("example.test") && !summary.contains("FROM_THE_ENVIRONMENT"),
        "{summary}"
    );
}

#[tokio::test]
async fn a_served_file_that_is_not_text_has_no_text_and_is_not_withheld() {
    let rig = rig();
    seed(&rig);
    // An image or any other binary resource: the config-server serves it, the agent hashes it, and there is no text to show.
    let mut file = served_fixture(None, false);
    file.file = np("receipt-sit1.bmp");
    file.role = domain::FileRole::Resource;
    rig.registry.inner.set_served(file);
    let mut c = viewer(&rig).await;
    let called = call(
        &mut c,
        "get_served_config",
        &json!({"swimlane": "sit1", "application": "tx-core", "tenant": "sit1", "file": "receipt-sit1.bmp"}),
    )
    .await;
    let data = called.ok();
    assert_eq!(data["text"], Value::Null);
    assert_eq!(data["content_withheld"], false);
    assert!(
        called.envelope()["summary"]
            .as_str()
            .unwrap()
            .contains("not text"),
        "{}",
        called.envelope()
    );
}

#[tokio::test]
async fn a_long_served_file_is_cut_at_the_cap_with_a_link_and_a_count() {
    let rig = rig();
    seed(&rig);
    let text: String = (0..3_000)
        .map(|n| format!("key{n}: value-number-{n}\n"))
        .collect();
    rig.registry.inner.set_served(served_fixture(Some(&text), false));
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_served_config", &request()).await;
    let envelope = called.envelope();
    assert_eq!(envelope["truncated"], true);
    assert!(
        envelope["web_link"]
            .as_str()
            .is_some_and(|l| l.starts_with("https://hub.example.test/swimlanes/sit1/served")),
        "{envelope}"
    );
    assert!(called.bytes() <= 16 * 1024, "{} bytes", called.bytes());
    let shown = envelope["data"]["text"].as_str().unwrap();
    assert!(shown.starts_with("key0: value-number-0\n"), "{shown:.80}");
    assert!(
        envelope["summary"].as_str().unwrap().contains("of 3000 lines"),
        "{envelope}"
    );
}

/// Round 2, review item 6: `docs/mcp-tools.md` caps file and doc text at "200 lines or 16 KB" by default, and `read_file` and
/// `read_doc` do. `get_served_config` fitted every line of the file to the cap, a binary search of renders each of which cut and
/// serialised the text so far: for the 2 MiB a file may be, that is work on the async thread out of all proportion to the 200
/// lines it can return. This holds what comes back for a file of that size (the work is bounded by `window`, tested in `redact`).
#[tokio::test]
async fn a_two_mib_served_file_shows_the_first_200_lines_and_says_how_many_there_are() {
    const TWO_MIB: usize = 2 * 1024 * 1024;
    let rig = rig();
    seed(&rig);
    // Short lines, so that the 16 KB cap is not what stops the text: 200 of them are about 2 KB. As much as the secret scan takes
    // (2 MiB; a larger text is withheld whole), to the last line that fits.
    let mut text = String::with_capacity(TWO_MIB);
    let mut n = 0_u32;
    loop {
        let line = format!("k{n}: v\n");
        if text.len() + line.len() > TWO_MIB {
            break;
        }
        text.push_str(&line);
        n += 1;
    }
    let total = n;
    assert!(text.len() > TWO_MIB - 16, "{}", text.len());
    rig.registry.inner.set_served(served_fixture(Some(&text), false));
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_served_config", &request()).await;
    let envelope = called.envelope();
    assert_eq!(envelope["truncated"], true);
    assert!(
        envelope["web_link"]
            .as_str()
            .is_some_and(|l| l.starts_with("https://hub.example.test/swimlanes/sit1/served")),
        "{envelope}"
    );
    assert!(
        envelope.get("next").is_none_or(|n| n.is_null()),
        "the contract has no cursor here: {envelope}"
    );
    assert!(called.bytes() <= 16 * 1024, "{} bytes", called.bytes());
    // Exactly the first 200 lines, as they are in the file.
    let shown = envelope["data"]["text"].as_str().unwrap();
    let expected: Vec<&str> = text.lines().take(200).collect();
    assert_eq!(shown.lines().count(), 200);
    assert_eq!(shown, expected.join("\n"));
    // And it says how many there are: the summary counts the whole file, not the part it looked at.
    let summary = envelope["summary"].as_str().unwrap();
    assert!(summary.contains(&format!("200 of {total} lines")), "{summary}");
}

/// The other bound of "200 lines or 16 KB": long lines. 600 lines of 3 KB are far inside 200 lines and far outside the cap, so the
/// cap decides, and what is shown is still a whole number of lines from the top.
#[tokio::test]
async fn long_lines_of_a_served_file_stop_at_the_16_kb_cap_on_a_line_boundary() {
    let rig = rig();
    seed(&rig);
    let line = |n: usize| format!("key{n}: {}\n", "x".repeat(3_000));
    let text: String = (0..600).map(line).collect();
    rig.registry.inner.set_served(served_fixture(Some(&text), false));
    let mut c = viewer(&rig).await;
    let called = call(&mut c, "get_served_config", &request()).await;
    let envelope = called.envelope();
    assert_eq!(envelope["truncated"], true);
    assert!(called.bytes() <= 16 * 1024, "{} bytes", called.bytes());
    let shown = envelope["data"]["text"].as_str().unwrap();
    let shown_lines = shown.lines().count();
    assert!(
        (1..=5).contains(&shown_lines),
        "{shown_lines} lines of 3 KB in 16 KB"
    );
    let expected: Vec<String> = (0..shown_lines).map(|n| line(n).trim_end().to_owned()).collect();
    assert_eq!(shown, expected.join("\n"));
    assert!(
        envelope["summary"]
            .as_str()
            .unwrap()
            .contains(&format!("{shown_lines} of 600 lines")),
        "{envelope}"
    );
}
