//! A12, S11: the links of a result stay on the configured origin, escape what they carry, and are never longer than the contract's
//! 512 characters.

use hub_mcp::config::PublicOrigin;
use hub_mcp::output::links::{LinkTemplates, MAX_LINK_LEN, WebLinks};
use proptest::prelude::*;

use crate::fix::*;

fn links() -> WebLinks {
    WebLinks::new(
        PublicOrigin::parse("https://hub.example.test").unwrap(),
        LinkTemplates::default(),
    )
}

#[test]
fn every_link_is_on_the_configured_origin() {
    let l = links();
    let all = [
        l.swimlane(&sid("sit1")),
        l.file(&sid("sit1"), &np("a/b.yml")),
        l.drift(&sid("sit1")),
        l.findings(Some(&sid("sit1"))),
        l.findings(None),
        l.restarts(&sid("sit1")),
        l.history(&sid("sit1"), &np("a/b.yml")),
        l.compare("nfs:sit1", "git:base@v1", &lf("a.yml")),
        l.compare_tree("nfs:sit1", "nfs:sit2"),
        l.grid(&lf("a.yml"), &["sit1", "sit2"]),
        l.search_text("timeout"),
        l.search_settings("timeout"),
        l.docs_search("timeout"),
        l.doc("/git/main/a.md"),
        l.effective(&sid("sit1"), &lf("a.yml"), "sit1"),
        l.served(&sid("sit1"), "tx-core", "sit1", &np("a.yml")),
        l.draft(&domain::DraftId::parse("d-1").unwrap()),
        l.pull_requests(&sid("sit1")),
    ];
    for link in all {
        let link = link.unwrap();
        assert!(link.starts_with("https://hub.example.test/"), "{link}");
        assert!(link.len() <= MAX_LINK_LEN);
        assert!(!link.contains(' ') && link.is_ascii(), "{link}");
    }
    assert_eq!(
        l.file(&sid("sit1"), &np("a/b.yml")).unwrap(),
        "https://hub.example.test/swimlanes/sit1/files?path=a%2Fb.yml"
    );
    assert_eq!(
        l.draft(&domain::DraftId::parse("d-1").unwrap()).unwrap(),
        "https://hub.example.test/drafts/d-1"
    );
}

#[test]
fn link_components_are_escaped() {
    let l = links();
    // A pattern from the model is hostile text: it must not be able to end the query, start another, or leave the origin.
    let link = l.search_text("a&b=c#frag %2F ../ \u{e9}\"'<>\n").unwrap();
    let (before, query) = link.split_once('?').unwrap();
    assert_eq!(before, "https://hub.example.test/search");
    let q = query.strip_prefix("mode=text&q=").unwrap();
    for raw in ['&', '#', ' ', '"', '\'', '<', '>', '\n', '\u{e9}', '='] {
        assert!(!q.contains(raw), "{raw:?} in {q}");
    }
    assert_eq!(q.matches('%').count(), q.matches("%").count());
    assert!(q.contains("%26") && q.contains("%23") && q.contains("%25"), "{q}");
    // A path keeps no slash in a query value (it is one value).
    assert!(
        l.file(&sid("sit1"), &np("a b/c#d.yml"))
            .unwrap()
            .ends_with("path=a%20b%2Fc%23d.yml")
    );
}

#[test]
fn a_template_with_an_unknown_placeholder_or_another_origin_is_refused() {
    let mut t = LinkTemplates::default();
    assert!(t.validate().is_ok());
    t.swimlane = "/s/{nope}".to_owned();
    assert!(t.validate().is_err());
    for bad in [
        "https://evil.example/{swimlane}",
        "//evil.example/{swimlane}",
        "s/{swimlane}",
        "/s/{swimlane",
        "/s/ {swimlane}",
        "",
    ] {
        let mut t = LinkTemplates::default();
        t.swimlane = bad.to_owned();
        assert!(t.validate().is_err(), "{bad:?}");
    }
    // A configured template changes the route and nothing else.
    let mut t = LinkTemplates::default();
    t.swimlane = "/lanes/{swimlane}/view".to_owned();
    t.validate().unwrap();
    let l = WebLinks::new(PublicOrigin::parse("https://hub.example.test").unwrap(), t);
    assert_eq!(
        l.swimlane(&sid("sit1")).unwrap(),
        "https://hub.example.test/lanes/sit1/view"
    );
}

proptest! {
    #[test]
    fn link_is_at_most_512_chars_prop(path in "[a-zA-Z0-9 _./&?#%-]{1,300}(/[a-z0-9]{1,200}){0,4}", pattern in ".{0,400}") {
        let l = links();
        if let Ok(path) = domain::NfsPath::parse(&path) {
            for link in [l.file(&sid("sit1"), &path), l.history(&sid("sit1"), &path)].into_iter().flatten() {
                prop_assert!(link.len() <= MAX_LINK_LEN);
                prop_assert!(link.starts_with("https://hub.example.test/"));
                prop_assert!(link.is_ascii() && !link.contains(' '));
            }
        }
        if let Some(link) = l.search_text(&pattern) {
            prop_assert!(link.len() <= MAX_LINK_LEN);
            prop_assert!(link.starts_with("https://hub.example.test/search?"));
            prop_assert!(link.is_ascii());
            prop_assert_eq!(link.matches('?').count(), 1);
        }
    }
}
