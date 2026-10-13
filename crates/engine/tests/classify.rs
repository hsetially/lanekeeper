//! T1 (S11, D79): `classify` puts a file in a class by its name and, for an extension-less file, its bytes. A file the
//! deny globs match is `denied` before anything else is looked at.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{FileClass, NfsPath};
use engine::{DEFAULT_DENY_GLOBS, DenyError, DenyMatcher, classify};
use proptest::prelude::*;

fn p(path: &str) -> NfsPath {
    NfsPath::parse(path).unwrap()
}

#[test]
fn classify_table() {
    use FileClass::{Binary, Denied, Structured, Text};
    let deny = DenyMatcher::default();
    let utf8: &[u8] = b"KEY=value\n";
    let rows: [(&str, Option<&[u8]>, FileClass); 31] = [
        // Structured, by extension, whatever the bytes or the case.
        ("a/b.yml", None, Structured),
        ("a/b.yaml", None, Structured),
        ("a/b.json", None, Structured),
        ("a/b.properties", None, Structured),
        ("a/B.YML", None, Structured),
        ("a/b.Properties", None, Structured),
        ("a/b.yml", Some(b"\x00\x01\x02"), Structured),
        // Text, by extension.
        ("doc/transform.xsl", None, Text),
        ("doc/x.xml", None, Text),
        ("doc/readme.txt", None, Text),
        ("doc/X.XSL", None, Text),
        // Binary: any other extension, even one that looks like text.
        ("img/receipt-ci_1.bmp", None, Binary),
        ("img/logo.png", Some(utf8), Binary),
        ("a/b.tar.gz", None, Binary),
        ("a/b.yml.bak", Some(utf8), Binary),
        ("a/b.csv", Some(utf8), Binary),
        // Extension-less: text only when the bytes are UTF-8 with no NUL.
        ("scripts/startup", Some(utf8), Text),
        ("scripts/startup", Some(b"\xEF\xBB\xBFKEY=value\r\n"), Text),
        ("scripts/startup", Some(b""), Text),
        ("scripts/startup", Some(b"caf\xE9"), Binary),
        ("scripts/startup", Some(b"a\x00b"), Binary),
        ("scripts/startup", Some(b"\xFF\xFEa\x00"), Binary),
        // Without the bytes there is nothing to decide with, and the safe answer is to compare by hash.
        ("scripts/startup", None, Binary),
        // A name that only starts with a dot, or ends in one, has no extension.
        (".profile", Some(utf8), Text),
        (".yml", Some(utf8), Text),
        ("scripts/startup.", Some(utf8), Text),
        // The tenant suffix of an extension-less file is part of its name.
        ("scripts/startup-sit1", Some(utf8), Text),
        // Denied wins over everything.
        ("keys/server.pem", Some(utf8), Denied),
        ("keys/server.pem", None, Denied),
        ("a/private.yml", Some(utf8), Denied),
        ("a/b/c/store.JKS", None, Denied),
    ];
    for (path, content, expected) in rows {
        assert_eq!(
            classify(&p(path), content, &deny),
            expected,
            "{path} with {content:?}"
        );
    }
    // Each of the seven default globs makes a file `denied`, with or without bytes.
    for path in [
        "a.jks",
        "a.p12",
        "a.pfx",
        "a.pem",
        "a.key",
        "a.keystore",
        "x/private/y.txt",
    ] {
        assert_eq!(classify(&p(path), None, &deny), Denied, "{path}");
        assert_eq!(classify(&p(path), Some(utf8), &deny), Denied, "{path}");
    }
}

#[test]
fn the_seven_default_deny_globs_each_deny() {
    // D79. The list is a contract with the agent, which applies the same globs; a change must be made on both sides.
    assert_eq!(
        DEFAULT_DENY_GLOBS,
        [
            "*.jks",
            "*.p12",
            "*.pfx",
            "*.pem",
            "*.key",
            "*.keystore",
            "*private*"
        ]
    );
    let deny = DenyMatcher::default();
    let denied = [
        "a.jks",
        "x/y/a.p12",
        "tls/a.pfx",
        "a.pem",
        "deep/er/and/deeper/a.key",
        "a.keystore",
        "my-private-config.yml",
        "private/readme.txt",
        "a/PRIVATE/b.txt",
        "x/A.PEM",
        "x/A.Key",
    ];
    for path in denied {
        assert!(deny.is_denied(&p(path)), "{path} should be denied");
    }
    // `a.jks/b.yml` is a file under a directory called `a.jks`: `*.jks` must end the path, and this path does not.
    let allowed = [
        "a.yml",
        "a.pe",
        "a.pem.txt",
        "pemfile",
        "keys.yml",
        "a/keystore",
        "priv/ate.yml",
        "a.jks/b.yml",
    ];
    for path in allowed {
        assert!(!deny.is_denied(&p(path)), "{path} should not be denied");
    }
}

#[test]
fn extra_globs_add_to_the_defaults_and_never_replace_them() {
    let deny = DenyMatcher::with_globs(["secrets/*", "exact-name"]).unwrap();
    assert!(deny.is_denied(&p("secrets/a.yml")));
    // A glob with a slash is matched against the whole path from the root, so it is found where it says. (`*` crosses
    // `/`, which is why the defaults find their files at any depth.)
    assert!(!deny.is_denied(&p("x/y/secrets/a.yml")));
    // A glob with no slash is also matched against the file name alone.
    assert!(deny.is_denied(&p("x/exact-name")));
    assert!(!deny.is_denied(&p("x/exact-name2")));
    // The defaults still apply.
    assert!(deny.is_denied(&p("a.pem")));
    // And none is dropped by an empty list.
    let none = DenyMatcher::with_globs(std::iter::empty::<&str>()).unwrap();
    assert!(none.is_denied(&p("a.pem")));
    assert!(!none.is_denied(&p("a.yml")));
}

#[test]
fn extra_globs_are_bounded_and_errors_do_not_echo_them() {
    let marker = "SECRET-MARKER-9f3a";
    // Not a glob.
    let err = DenyMatcher::with_globs([format!("{marker}[").as_str()]).unwrap_err();
    assert_eq!(err, DenyError::InvalidGlob);
    for text in [err.to_string(), format!("{err:?}")] {
        assert!(!text.contains(marker), "{text}");
    }
    // Too long.
    let long = "a".repeat(513);
    assert_eq!(
        DenyMatcher::with_globs([long.as_str()]).unwrap_err(),
        DenyError::GlobTooLong
    );
    let at_limit = "a".repeat(512);
    assert!(DenyMatcher::with_globs([at_limit.as_str()]).is_ok());
    // Too many.
    let many: Vec<String> = (0..=1_000).map(|i| format!("x{i}")).collect();
    assert_eq!(
        DenyMatcher::with_globs(many.iter().map(String::as_str)).unwrap_err(),
        DenyError::TooManyGlobs
    );
    assert!(DenyMatcher::with_globs(many.iter().take(1_000).map(String::as_str)).is_ok());
}

proptest! {
    /// Denied is decided by the path alone: no bytes change it, and every other class is reached only when it is not.
    #[test]
    fn denied_iff_the_matcher_says_so_prop(
        name in "[a-z][a-zA-Z0-9_.-]{0,11}",
        dir in "[a-z]{0,6}",
        content in prop::option::of(prop::collection::vec(any::<u8>(), 0..32)),
    ) {
        let path = if dir.is_empty() { name } else { format!("{dir}/{name}") };
        let path = p(&path);
        let deny = DenyMatcher::default();
        let class = classify(&path, content.as_deref(), &deny);
        prop_assert_eq!(class == FileClass::Denied, deny.is_denied(&path));
        // The same inputs give the same answer.
        prop_assert_eq!(class, classify(&path, content.as_deref(), &deny));
    }
}
